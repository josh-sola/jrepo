mod db;
mod names;

use chrono::{Local, TimeZone};
use clap::{Parser, Subcommand};
use db::Result;
use rusqlite::Connection;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(250);

const GUIDE: &str = include_str!("../README.md");

const LISTEN_HELP: &str = "\
From a Claude Code agent, start a Monitor:
  command:    pubsub listen <topic> --user <your-name>
  timeout_ms: 1800000
When it expires, start it again with the same command. --user saves your place, so nothing is missed.
A message ends at its [/#id] line. [closed <topic>] means the topic is gone.
Run `pubsub guide` for more.";

#[derive(Parser)]
#[command(
    about = "A local message bus for agents",
    after_help = "Agents: run `pubsub guide` to learn how to listen with a Monitor."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a topic and print its name
    Create,
    /// Send a message read from a file or stdin
    Send {
        topic: String,
        #[arg(long)]
        user: String,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Print messages as they arrive
    #[command(after_help = LISTEN_HELP)]
    Listen {
        topic: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        from_start: bool,
    },
    /// Close a topic and delete its messages
    Close { topic: String },
    /// Print the full guide for agents
    Guide,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("pubsub: {err}");
        std::process::exit(1);
    }
}

fn db_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PUBSUB_DB") {
        return Ok(path.into());
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/share/pubsub/pubsub.db"))
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let open = || db::open(&db_path()?);
    match cli.command {
        Command::Create => {
            let topic = db::create_topic(&open()?)?;
            println!("{topic}");
            eprintln!(
                "Listen: pubsub listen {topic} --user <your-name>\n\
                 Send:   pubsub send {topic} --user <your-name> < message.txt\n\
                 Run `pubsub guide` for how to listen with a Monitor."
            );
        }
        Command::Send { topic, user, file } => {
            let body = read_body(file)?;
            println!("{}", db::send(&open()?, &topic, &user, &body)?);
        }
        Command::Listen {
            topic,
            user,
            from_start,
        } => listen(&open()?, &topic, user.as_deref(), from_start)?,
        Command::Close { topic } => db::close(&open()?, &topic)?,
        Command::Guide => print!("{GUIDE}"),
    }
    Ok(())
}

fn read_body(file: Option<PathBuf>) -> Result<String> {
    let bytes = match file {
        Some(path) => std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
        None => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            buf
        }
    };
    let body = String::from_utf8(bytes).map_err(|_| "message body is not valid UTF-8")?;
    if body.is_empty() {
        return Err("message body is empty".into());
    }
    Ok(body)
}

fn listen(conn: &Connection, topic: &str, user: Option<&str>, from_start: bool) -> Result<()> {
    match db::topic_closed(conn, topic)? {
        None => return Err(format!("unknown topic {topic}").into()),
        Some(true) => return Err(format!("topic {topic} is closed").into()),
        Some(false) => {}
    }
    let mut last = if from_start {
        0
    } else {
        match user
            .map(|u| db::cursor(conn, topic, u))
            .transpose()?
            .flatten()
        {
            Some(saved) => saved,
            None => db::max_id(conn, topic)?,
        }
    };
    // Save the start point now, so a restart after a quiet expiry still sees messages sent in the gap.
    if let Some(u) = user {
        db::save_cursor(conn, topic, u, last)?;
    }
    let mut out = std::io::stdout();
    loop {
        for msg in db::messages_after(conn, topic, last)? {
            // Own messages still advance the cursor so a restart does not revisit them.
            if user != Some(msg.user.as_str()) {
                let time = Local
                    .timestamp_opt(msg.at, 0)
                    .single()
                    .map(|t| t.format("%H:%M:%S").to_string())
                    .unwrap_or_default();
                let newline = if msg.data.ends_with('\n') { "" } else { "\n" };
                write!(
                    out,
                    "[#{id} {user} {time}]\n{data}{newline}[/#{id}]\n",
                    id = msg.id,
                    user = msg.user,
                    data = msg.data,
                )?;
                out.flush()?;
            }
            last = msg.id;
            if let Some(u) = user {
                db::save_cursor(conn, topic, u, last)?;
            }
        }
        if db::topic_closed(conn, topic)? != Some(false) {
            writeln!(out, "[closed {topic}]")?;
            out.flush()?;
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
