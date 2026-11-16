//! Which messages of a run the pipeline acknowledged, so an input changes the server only for
//! those and only once the run's last answer is in.

use std::time::Duration;

use ekiben_core::AckResponder;
use tokio::sync::mpsc;

/// How long the end of a run waits for the answers still out before it leaves the rest as they
/// are on the server (read again next run).
pub(crate) const ACK_WAIT: Duration = Duration::from_secs(60);

/// The answers to the messages of one run: `key` is what the input acts on when it is acked.
pub(crate) struct Acks<K> {
    tx: mpsc::UnboundedSender<(K, bool)>,
    rx: mpsc::UnboundedReceiver<(K, bool)>,
    outstanding: usize,
}

impl<K: Send + Sync + 'static> Acks<K> {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx,
            outstanding: 0,
        }
    }

    /// The responder of one message: an ack sends `(key, true)`, a nack `(key, false)`.
    pub(crate) fn responder(&mut self, key: K) -> AckResponder {
        self.outstanding += 1;
        let tx = self.tx.clone();
        AckResponder::from_fn(move |err| {
            let _ = tx.send((key, err.is_none()));
        })
    }

    /// The keys acked so far, without waiting.
    pub(crate) fn ready(&mut self) -> Vec<K> {
        let mut acked = Vec::new();
        while let Ok((key, ok)) = self.rx.try_recv() {
            self.outstanding -= 1;
            if ok {
                acked.push(key);
            }
        }
        acked
    }

    /// The keys acked, waiting up to `wait` for the answers still out.
    pub(crate) async fn all(&mut self, wait: Duration) -> Vec<K> {
        let mut acked = self.ready();
        let deadline = tokio::time::Instant::now() + wait;
        while self.outstanding > 0 {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some((key, ok))) => {
                    self.outstanding -= 1;
                    if ok {
                        acked.push(key);
                    }
                }
                _ => break,
            }
        }
        acked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::BatchError;

    #[tokio::test]
    async fn only_acked_keys_come_back_and_a_lost_answer_is_not_waited_for_past_the_limit() {
        let mut acks = Acks::new();
        acks.responder("a").ack();
        acks.responder("b")
            .nack(BatchError::new(Vec::new(), "output refused"));
        assert_eq!(acks.ready(), vec!["a"]);
        let late = acks.responder("c");
        let _lost = acks.responder("d");
        tokio::spawn(async move { late.ack() });
        assert_eq!(acks.all(Duration::from_millis(200)).await, vec!["c"]);
    }
}
