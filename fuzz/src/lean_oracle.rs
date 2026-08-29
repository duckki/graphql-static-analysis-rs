//! Persistent native Lean-oracle transport.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub const LEAN_ORACLE_ENV: &str = "GRAPHQL_STATIC_ANALYSIS_LEAN_ORACLE";
pub const ALLOW_STALE_ORACLE_ENV: &str = "GRAPHQL_STATIC_ANALYSIS_ALLOW_STALE_LEAN_ORACLE";

pub struct NativeLeanOracle {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl NativeLeanOracle {
    pub fn from_env() -> Self {
        let executable = std::env::var_os(LEAN_ORACLE_ENV)
            .unwrap_or_else(|| panic!("{LEAN_ORACLE_ENV} must name a native TreeSummary oracle"));
        Self::new(executable)
    }

    pub fn from_env_with_model_commit(expected_commit: &str) -> Self {
        let executable = std::env::var_os(LEAN_ORACLE_ENV)
            .unwrap_or_else(|| panic!("{LEAN_ORACLE_ENV} must name a native TreeSummary oracle"));
        Self::new_with_model_commit(executable, expected_commit)
    }

    pub fn new_with_model_commit(executable: impl AsRef<Path>, expected_commit: &str) -> Self {
        validate_model_commit(executable.as_ref(), expected_commit);
        Self::new(executable)
    }

    pub fn new(executable: impl AsRef<Path>) -> Self {
        let mut child = Command::new(executable.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|error| {
                panic!(
                    "start Lean oracle {}: {error}",
                    executable.as_ref().display()
                )
            });
        Self {
            stdin: child.stdin.take().expect("open Lean oracle stdin"),
            stdout: BufReader::new(child.stdout.take().expect("open Lean oracle stdout")),
            child,
        }
    }

    pub fn query(&mut self, request_id: &str, payload: &str) -> String {
        writeln!(self.stdin, "{payload}").expect("write Lean oracle request");
        self.stdin.flush().expect("flush Lean oracle request");

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("read Lean oracle response");
        assert!(!line.is_empty(), "Lean oracle exited before responding");
        let (actual_id, result) = line
            .trim_end()
            .split_once('=')
            .unwrap_or_else(|| panic!("unexpected Lean oracle output: {line}"));
        assert_eq!(actual_id, request_id, "Lean oracle response id");
        result.to_string()
    }
}

fn validate_model_commit(executable: &Path, expected_commit: &str) {
    let mut sidecar: OsString = executable.as_os_str().to_owned();
    sidecar.push(".model-commit");
    let actual_commit = fs::read_to_string(Path::new(&sidecar))
        .unwrap_or_else(|error| panic!("read Lean oracle model commit sidecar: {error}"));
    let actual_commit = actual_commit.trim();
    if actual_commit == expected_commit {
        return;
    }
    if std::env::var_os(ALLOW_STALE_ORACLE_ENV).is_some() {
        eprintln!(
            "warning: Lean oracle model commit is {actual_commit}, expected {expected_commit}"
        );
        return;
    }
    panic!(
        "Lean oracle model commit is {actual_commit}, expected {expected_commit}; \
         set {ALLOW_STALE_ORACLE_ENV}=1 to override deliberately"
    );
}

impl Drop for NativeLeanOracle {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}
