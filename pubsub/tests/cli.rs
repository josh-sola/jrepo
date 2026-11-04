use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const DEADLINE: Duration = Duration::from_secs(10);

struct Env {
    dir: TempDir,
}

struct Output {
    ok: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

struct Listener {
    child: Child,
    lines: Receiver<String>,
}

impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pubsub"));
        cmd.args(args)
            .env("PUBSUB_DB", self.dir.path().join("test.db"));
        cmd
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        Output {
            ok: out.status.success(),
            code: out.status.code(),
            stdout: String::from_utf8(out.stdout).unwrap(),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    }

    fn create(&self) -> String {
        let out = self.run(&["create"], "");
        assert!(out.ok, "{}", out.stderr);
        out.stdout.trim().to_string()
    }

    fn send(&self, topic: &str, user: &str, body: &str) -> Output {
        self.run(&["send", topic, "--user", user], body)
    }

    fn listen(&self, args: &[&str]) -> Listener {
        let mut child = self
            .command(&[&["listen"], args].concat())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Listener { child, lines }
    }
}

impl Listener {
    fn next_line(&self) -> Option<String> {
        self.lines.recv_timeout(DEADLINE).ok()
    }

    fn expect_line(&self) -> String {
        self.next_line().expect("timed out waiting for output")
    }

    fn no_output_for(&self, wait: Duration) -> bool {
        matches!(
            self.lines.recv_timeout(wait),
            Err(RecvTimeoutError::Timeout)
        )
    }

    fn wait_exit(&mut self) -> Option<i32> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            assert!(start.elapsed() < DEADLINE, "listener did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_frame_header(line: &str, id: &str, user: &str) -> bool {
    let prefix = format!("[#{id} {user} ");
    line.starts_with(&prefix)
        && line.ends_with(']')
        && line[prefix.len()..line.len() - 1].matches(':').count() == 2
}

fn assert_frame(l: &Listener, id: &str, user: &str, body: &[&str]) {
    let header = l.expect_line();
    assert!(is_frame_header(&header, id, user), "bad header: {header}");
    for expected in body {
        assert_eq!(&l.expect_line(), expected);
    }
    assert_eq!(l.expect_line(), format!("[/#{id}]"));
}

fn wait_for_start(env: &Env, topic: &str, args: &[&str]) -> Listener {
    let l = env.listen(&[&[topic], args].concat());
    // Listen has no ready signal; give it time to fix its start point.
    std::thread::sleep(Duration::from_millis(500));
    l
}

#[test]
fn create_returns_distinct_three_word_names() {
    let env = Env::new();
    let a = env.create();
    let b = env.create();
    for name in [&a, &b] {
        let parts: Vec<_> = name.split('-').collect();
        assert_eq!(parts.len(), 3, "{name}");
        assert!(
            parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase()))
        );
    }
    assert_ne!(a, b);
}

#[test]
fn send_reads_stdin_and_file() {
    let env = Env::new();
    let topic = env.create();
    let first = env.send(&topic, "alice", "hello\n");
    assert!(first.ok, "{}", first.stderr);
    let first_id: i64 = first.stdout.trim().parse().unwrap();

    let path = env.dir.path().join("body.txt");
    std::fs::write(&path, "from file").unwrap();
    let second = env.run(
        &[
            "send",
            &topic,
            "--user",
            "alice",
            "--file",
            path.to_str().unwrap(),
        ],
        "",
    );
    assert!(second.ok, "{}", second.stderr);
    assert!(second.stdout.trim().parse::<i64>().unwrap() > first_id);

    let l = env.listen(&[&topic, "--from-start"]);
    assert_frame(&l, &first_id.to_string(), "alice", &["hello"]);
    assert_frame(&l, second.stdout.trim(), "alice", &["from file"]);
    l.stop();
}

#[test]
fn send_rejects_empty_body() {
    let env = Env::new();
    let topic = env.create();
    let out = env.send(&topic, "alice", "");
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.starts_with("pubsub: "), "{}", out.stderr);
}

#[test]
fn send_to_closed_or_unknown_topic_fails() {
    let env = Env::new();
    let topic = env.create();
    assert!(env.run(&["close", &topic], "").ok);
    let out = env.send(&topic, "alice", "hi");
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.contains("closed"), "{}", out.stderr);
    assert_eq!(env.send("no-such-topic", "alice", "hi").code, Some(1));
}

#[test]
fn listen_prints_only_new_messages() {
    let env = Env::new();
    let topic = env.create();
    assert!(env.send(&topic, "alice", "old").ok);
    let l = wait_for_start(&env, &topic, &[]);
    let id = env.send(&topic, "alice", "new\n").stdout.trim().to_string();
    assert_frame(&l, &id, "alice", &["new"]);
    l.stop();
}

#[test]
fn listen_from_start_replays_history() {
    let env = Env::new();
    let topic = env.create();
    let one = env.send(&topic, "alice", "one").stdout.trim().to_string();
    let two = env
        .send(&topic, "bob", "two\nlines")
        .stdout
        .trim()
        .to_string();
    let l = env.listen(&[&topic, "--from-start"]);
    assert_frame(&l, &one, "alice", &["one"]);
    assert_frame(&l, &two, "bob", &["two", "lines"]);
    l.stop();
}

#[test]
fn listen_with_user_hides_own_messages() {
    let env = Env::new();
    let topic = env.create();
    let l = wait_for_start(&env, &topic, &["--user", "bob"]);
    assert!(env.send(&topic, "bob", "mine").ok);
    let id = env
        .send(&topic, "alice", "theirs")
        .stdout
        .trim()
        .to_string();
    assert_frame(&l, &id, "alice", &["theirs"]);
    assert!(l.no_output_for(Duration::from_millis(600)));
    l.stop();
}

#[test]
fn listen_resumes_after_quiet_restart() {
    let env = Env::new();
    let topic = env.create();
    wait_for_start(&env, &topic, &["--user", "bob"]).stop();

    let id = env.send(&topic, "alice", "gap").stdout.trim().to_string();
    let l = env.listen(&[&topic, "--user", "bob"]);
    assert_frame(&l, &id, "alice", &["gap"]);
    l.stop();
}

#[test]
fn listen_resumes_from_saved_cursor() {
    let env = Env::new();
    let topic = env.create();
    let l = wait_for_start(&env, &topic, &["--user", "bob"]);
    let first = env.send(&topic, "alice", "first").stdout.trim().to_string();
    assert_frame(&l, &first, "alice", &["first"]);
    // The cursor is saved after the message prints; wait for the listener to go idle.
    std::thread::sleep(Duration::from_millis(600));
    l.stop();

    let second = env
        .send(&topic, "alice", "second")
        .stdout
        .trim()
        .to_string();
    let l = env.listen(&[&topic, "--user", "bob"]);
    assert_frame(&l, &second, "alice", &["second"]);
    l.stop();
}

#[test]
fn listen_exits_when_topic_closes() {
    let env = Env::new();
    let topic = env.create();
    let mut l = wait_for_start(&env, &topic, &[]);
    assert!(env.run(&["close", &topic], "").ok);
    assert_eq!(l.expect_line(), format!("[closed {topic}]"));
    assert_eq!(l.wait_exit(), Some(0));
}

#[test]
fn listen_on_unknown_topic_exits_with_error() {
    let env = Env::new();
    let out = env.run(&["listen", "no-such-topic"], "");
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.starts_with("pubsub: "), "{}", out.stderr);
}

#[test]
fn close_unknown_topic_fails_and_repeat_close_succeeds() {
    let env = Env::new();
    assert_eq!(env.run(&["close", "no-such-topic"], "").code, Some(1));
    let topic = env.create();
    assert!(env.run(&["close", &topic], "").ok);
    assert!(env.run(&["close", &topic], "").ok);
}

#[test]
fn guide_prints_monitor_setup() {
    let env = Env::new();
    let out = env.run(&["guide"], "");
    assert!(out.ok);
    assert!(out.stdout.contains("timeout_ms"));
}
