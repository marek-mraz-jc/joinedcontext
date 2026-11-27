//! `jcctl validate` when the process's environment is wrong: `JC_ENVIRONMENT` naming an overlay
//! nobody declared (CC-73) and a temporary directory that cannot be written. Both are
//! process-wide, so every test of this binary holds one lock while it sets them.

use jc_manifest::assemble::Directories;
use jc_manifest::validate::{run, run_assembled, run_project};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static PROCESS: Mutex<()> = Mutex::new(());

/// The lock a test holds from its first line, since even making a directory reads `TMPDIR`.
fn process() -> MutexGuard<'static, ()> {
    PROCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sets a variable for one test and puts the process back as it was, whether the test passes or
/// not; declared after the lock, it is dropped before it.
struct Set {
    name: &'static str,
    was: Option<std::ffi::OsString>,
}

impl Set {
    fn new(name: &'static str, value: &std::ffi::OsStr) -> Self {
        let was = std::env::var_os(name);
        std::env::set_var(name, value);
        Self { name, was }
    }
}

impl Drop for Set {
    fn drop(&mut self) {
        match &self.was {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}

fn temp_dir(test: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("jcctl-env-{test}-{}-{now}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the directory");
    dir
}

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("relative path has a parent"))
        .expect("create parent directory");
    std::fs::write(path, body).expect("write the file");
}

const ORG: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata: { name: banskabystrica, namespace: org }\nspec: { domain: banskabystrica.sk, locales: [sk], defaultLocale: sk }\n";
const PROJECT: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: banskabystrica\n";

/// A project repository of layout 2 and the organization that registers it.
fn fixtures(test: &str) -> (PathBuf, PathBuf) {
    let project = temp_dir(&format!("{test}-project"));
    write(&project, ".jc/layout", "2\n");
    write(&project, "project.yaml", PROJECT);
    let org = temp_dir(&format!("{test}-org"));
    write(&org, "org.yaml", ORG);
    write(&org, ".jc/layout", "2\n");
    write(
        &org,
        "projects/ovzdusie.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: banskabystrica\n  repository: { name: ovzdusie }\n  ref: main\n",
    );
    (project, org)
}

/// CC-73: an overlay nobody declared is one finding on `environments`, whichever way the
/// repository is checked, and the check stops there.
#[test]
fn an_environment_no_manifest_declares_is_one_finding_on_every_entry_point() {
    let _process = process();
    let (project, org) = fixtures("no-such-environment");
    let layout_1 = temp_dir("no-such-environment-single");
    write(&layout_1, "org.yaml", ORG);
    let _env = Set::new("JC_ENVIRONMENT", "nowhere".as_ref());
    let resolver = Directories(BTreeMap::from([("ovzdusie".to_owned(), project.clone())]));

    for (what, report) in [
        (
            "project",
            run_project(&project, None, "example.org", &BTreeMap::new()),
        ),
        ("organization", run_assembled(&org, &resolver)),
        ("layout 1", run(&layout_1)),
    ] {
        assert_eq!(report.checked, 0, "{what}");
        assert_eq!(report.findings.len(), 1, "{what}: {:?}", report.findings);
        let finding = &report.findings[0];
        assert_eq!(finding.path, PathBuf::from("environments"), "{what}");
        assert_eq!((finding.document, finding.line), (1, 1), "{what}");
        assert!(
            finding.message.contains("JC_ENVIRONMENT names `nowhere`"),
            "{what}: {}",
            finding.message
        );
    }
}

/// A project that cannot be staged because the temporary directory is a file is one finding on
/// `project.yaml` naming the path it could not write.
#[test]
fn a_project_that_cannot_be_staged_is_one_finding_on_its_file() {
    let _process = process();
    let (project, _) = fixtures("unwritable-temp");
    let blocker = temp_dir("unwritable-temp-blocker").join("a-file");
    std::fs::write(&blocker, "not a directory").expect("write the blocker");
    let _tmp = Set::new("TMPDIR", blocker.as_os_str());

    let report = run_project(&project, None, "example.org", &BTreeMap::new());
    assert_eq!(report.checked, 0);
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].path, PathBuf::from("project.yaml"));
    assert!(
        report.findings[0].message.contains("a-file"),
        "{}",
        report.findings[0].message
    );
}
