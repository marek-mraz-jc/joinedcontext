//! Log capture for the lib tests.
//!
//! One process-global subscriber writes each event to the buffer of the thread that captures it.
//! A scoped `set_default` per test raced tracing-core's callsite interest cache: when another test
//! thread registered a callsite while the scoped subscriber was being registered, the callsite
//! kept `Interest::never` and the capturing test read an empty log (T-7613, ci 45482851985 and
//! 45500747555). The global subscriber's filter is a `dynamic_filter_fn`, which answers
//! `sometimes` for every callsite, so tracing asks it per event; a plain `filter_fn` caches its
//! first answer as the callsite's interest, and a callsite a non-capturing thread reached first
//! stays `never`. `logged` rebuilds the cache for callsites registered before the install.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, Once};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;

type Buffer = Arc<Mutex<Vec<u8>>>;

thread_local! {
    /// The buffer this thread captures into and the most verbose level it keeps.
    static CAPTURE: RefCell<Option<(Buffer, LevelFilter)>> = const { RefCell::new(None) };
}

/// Installs the global routing subscriber once per process and rebuilds the interest cache, which
/// also settles a callsite whose registration raced the install.
pub(crate) fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let capturing = tracing_subscriber::filter::dynamic_filter_fn(|meta, _| {
            CAPTURE.with(|capture| {
                capture
                    .borrow()
                    .as_ref()
                    .is_some_and(|(_, level)| level >= meta.level())
            })
        });
        let subscriber = tracing_subscriber::registry().with(capturing).with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(|| Routed),
        );
        tracing::subscriber::set_global_default(subscriber)
            .expect("no other global subscriber in the lib tests");
    });
    tracing::callsite::rebuild_interest_cache();
}

/// Everything logged at `level` or above on this thread while `run` runs, as text.
pub(crate) async fn logged<F: std::future::Future>(
    level: LevelFilter,
    run: F,
) -> (F::Output, String) {
    install();
    let buffer = Buffer::default();
    let previous = CAPTURE.with(|capture| capture.replace(Some((Arc::clone(&buffer), level))));
    let restore = Restore(previous);
    let out = run.await;
    drop(restore);
    let text =
        String::from_utf8_lossy(&buffer.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    (out, text)
}

/// Puts back the capture that was active before, also when `run` panics.
struct Restore(Option<(Buffer, LevelFilter)>);

impl Drop for Restore {
    fn drop(&mut self) {
        let previous = self.0.take();
        CAPTURE.with(|capture| *capture.borrow_mut() = previous);
    }
}

/// Writes into the buffer of the capturing thread.
struct Routed;

impl std::io::Write for Routed {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        CAPTURE.with(|capture| {
            if let Some((buffer, _)) = capture.borrow().as_ref() {
                buffer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(bytes);
            }
        });
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_capture_keeps_its_level_and_its_own_thread_only() {
        let ((), text) = logged(LevelFilter::INFO, async {
            tracing::info!(client = "praha", "kept");
            tracing::debug!("below the level");
            std::thread::spawn(|| tracing::info!("another thread"))
                .join()
                .expect("thread");
        })
        .await;
        assert!(
            text.contains("kept") && text.contains("client=\"praha\""),
            "{text}"
        );
        assert!(!text.contains("below the level"), "{text}");
        assert!(!text.contains("another thread"), "{text}");
        let ((), outside) = logged(LevelFilter::TRACE, async {}).await;
        assert_eq!(outside, "");
    }

    #[tokio::test]
    async fn a_nested_capture_hands_the_outer_one_back() {
        let (inner, outer) = logged(LevelFilter::INFO, async {
            tracing::info!("outer before");
            let ((), inner) = logged(LevelFilter::TRACE, async { tracing::trace!("inner") }).await;
            tracing::info!("outer after");
            inner
        })
        .await;
        assert!(
            inner.contains("inner") && !inner.contains("outer"),
            "{inner}"
        );
        assert!(
            outer.contains("outer before") && outer.contains("outer after"),
            "{outer}"
        );
        assert!(!outer.contains("inner"), "{outer}");
    }

    /// A write from a thread that captures nothing goes nowhere and still reports its bytes; a
    /// flush has nothing to do; a buffer a panicking writer poisoned still takes and gives text.
    #[tokio::test]
    async fn a_stray_write_a_flush_and_a_poisoned_buffer_are_harmless() {
        use std::io::Write;
        assert_eq!(Routed.write(b"nobody listens").expect("a write"), 14);
        Routed.flush().expect("a flush");
        let buffer = Buffer::default();
        let poisoned = Arc::clone(&buffer);
        let _ = std::thread::spawn(move || {
            let _held = poisoned.lock().expect("the lock");
            std::panic::panic_any("a panic while holding the buffer");
        })
        .join();
        assert!(buffer.is_poisoned());
        let previous = CAPTURE.with(|c| c.replace(Some((Arc::clone(&buffer), LevelFilter::INFO))));
        Routed.write(b"kept").expect("a write");
        CAPTURE.with(|c| *c.borrow_mut() = previous);
        assert_eq!(&*buffer.lock().unwrap_or_else(|e| e.into_inner()), b"kept");
        let ((), text) = logged(LevelFilter::INFO, async {
            let buffer = CAPTURE.with(|c| c.borrow().as_ref().map(|(b, _)| Arc::clone(b)));
            let held = buffer.expect("a capture");
            let _ = std::thread::spawn(move || {
                let _held = held.lock().expect("the lock");
                std::panic::panic_any("a panic while holding the capture");
            })
            .join();
            tracing::info!("after the panic");
        })
        .await;
        assert!(text.contains("after the panic"), "{text}");
    }
}
