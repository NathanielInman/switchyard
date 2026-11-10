//! `yard`: the Switchyard daemon, CLI, waybar module and in-unit runner.

mod cli;
mod daemon;
mod history;
mod runlog;
mod runner;
mod setup;
mod spec;
mod supervise;
mod sys;

use anyhow::Result;

const HELP: &str = "\
yard: a dispatch board for the scripts you keep writing

usage: yard <command> [args]

  toggle [--center]        show or hide the board (what the bar click runs)
  show [--center] | hide   show or hide the board explicitly
  ls [--json]              list tasks with their state
  run <id> [-p k=v]... [--variant name] [--clean]
                           run a task
  stop <id>                stop a task or take it out of the queue
  log <id> [-f] [-n N]     print or follow a task's log
  ack [id]                 acknowledge failures
  status                   one-line summary
  history [id] [--json]    what ran, folded into episodes by day
  add [dir]                scaffold a .yard.toml for a repo
  reload                   re-read config and manifests now
  bar                      waybar module: stream the aggregate state
  setup [--uninstall]      start Switchyard at login, write a starter config
  doctor                   check the setup
  daemon                   run the daemon (the user unit does this)
  version
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(unix)]
    if args.first().map(String::as_str) != Some("daemon") {
        // Behave like a normal CLI when piped into `head`.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    let code = match dispatch(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yard: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn dispatch(args: &[String]) -> Result<i32> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];
    match cmd {
        "daemon" => daemon::run().map(|_| 0),
        "__build" => runner::build(rest.first().map(String::as_str).unwrap_or("")),
        "__launch" => runner::launch(rest.first().map(String::as_str).unwrap_or("")),
        "toggle" => cli::board(cli::BoardCmd::Toggle, rest),
        "show" => cli::board(cli::BoardCmd::Show, rest),
        "hide" => cli::board(cli::BoardCmd::Hide, rest),
        "ls" | "list" => cli::ls(rest),
        "run" => cli::run(rest),
        "stop" => cli::stop(rest),
        "log" | "logs" => cli::log(rest),
        "ack" => cli::ack(rest),
        "status" => cli::status(),
        "history" => cli::history(rest),
        "add" => cli::add(rest),
        "reload" => cli::simple(yard_core::protocol::Request::Reload),
        "bar" => cli::bar(),
        "doctor" => cli::doctor(),
        "setup" => setup::run(rest),
        // Sends one raw JSON request and prints the reply (tests, debugging).
        "__request" => {
            let req: yard_core::protocol::Request = serde_json::from_str(rest.first().map(String::as_str).unwrap_or(""))?;
            println!("{}", serde_json::to_string(&yard_core::protocol::request(&req)?)?);
            Ok(0)
        }
        "version" | "--version" | "-V" => {
            println!("yard {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(0)
        }
        other => {
            eprint!("yard: unknown command \"{other}\"\n\n{HELP}");
            Ok(2)
        }
    }
}
