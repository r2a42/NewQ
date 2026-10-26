//! Projector control over PJLink (class 1): the commands, the password
//! digest, reading the replies, and the conversation itself.
//!
//! A PJLink projector speaks first: `PJLINK 0` when it has no password, or
//! `PJLINK 1 <random>` when it has, in which case the first command carries
//! `md5(random + password)` in front of it. Each command is one line starting
//! `%1` and ending with a carriage return, and is answered with one line.

use crate::show::{ProjectorAction, ProjectorOut};
use md5::{Digest, Md5};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// How long to wait for a projector to answer (they are slow when waking up).
const TIMEOUT: Duration = Duration::from_secs(4);

/// The command line for `action`, without the password digest or the carriage return.
pub fn command_text(action: ProjectorAction) -> String {
    match action {
        ProjectorAction::PowerOn => "%1POWR 1".into(),
        ProjectorAction::PowerOff => "%1POWR 0".into(),
        // AVMT's first digit is what is muted (1 video) and the second on or off.
        ProjectorAction::ShutterClosed => "%1AVMT 11".into(),
        ProjectorAction::ShutterOpen => "%1AVMT 10".into(),
        ProjectorAction::Input(code) => format!("%1INPT {code}"),
    }
}

/// The digest a projector wants in front of the first command.
pub fn digest(random: &str, password: &str) -> String {
    let mut h = Md5::new();
    h.update(random.as_bytes());
    h.update(password.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Greeting {
    Open,
    /// Needs a password; this is the projector's random number.
    Password(String),
}

pub fn parse_greeting(line: &str) -> Result<Greeting, String> {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    if !parts
        .next()
        .is_some_and(|p| p.eq_ignore_ascii_case("PJLINK"))
    {
        return Err(format!("that isn't a PJLink projector (it said “{line}”)"));
    }
    match (parts.next(), parts.next()) {
        (Some("0"), _) => Ok(Greeting::Open),
        (Some("1"), Some(random)) => Ok(Greeting::Password(random.to_string())),
        (Some("ERRA"), _) => Err("the projector turned the connection away (password)".into()),
        _ => Err(format!("unexpected greeting “{line}”")),
    }
}

/// What a reply to `command` (the `%1XXXX` part) means: Ok if the projector did it.
pub fn parse_reply(command: &str, line: &str) -> Result<(), String> {
    let line = line.trim();
    let name = command.get(2..6).unwrap_or(command);
    let Some(rest) = line.strip_prefix("%1").and_then(|r| r.strip_prefix(name)) else {
        return if line.eq_ignore_ascii_case("PJLINK ERRA") {
            Err("wrong PJLink password".into())
        } else {
            Err(format!("unexpected reply “{line}”"))
        };
    };
    match rest.strip_prefix('=').unwrap_or(rest) {
        "OK" => Ok(()),
        "ERR1" => Err("the projector doesn't know that command".into()),
        "ERR2" => Err("the projector doesn't accept that setting".into()),
        "ERR3" => Err("the projector can't do that just now (warming up or cooling down?)".into()),
        "ERR4" => Err("the projector reports a fault".into()),
        "ERRA" => Err("wrong PJLink password".into()),
        other => Err(format!("unexpected reply “{other}”")),
    }
}

/// One line from the projector. PJLink ends its lines with a carriage return
/// alone, which `read_line` would wait past.
fn read_cr_line<R: BufRead>(reader: &mut R) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    reader.read_until(b'\r', &mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Talk to a projector over `stream`: read its greeting, send the command and
/// check the answer.
pub fn converse<S: Read + Write>(
    stream: S,
    password: &str,
    action: ProjectorAction,
) -> Result<(), String> {
    let command = command_text(action);
    let mut reader = BufReader::new(stream);
    let line = read_cr_line(&mut reader).map_err(|e| format!("no greeting: {e}"))?;
    let prefix = match parse_greeting(&line)? {
        Greeting::Open => String::new(),
        Greeting::Password(random) => {
            if password.is_empty() {
                return Err("it wants a PJLink password".into());
            }
            digest(&random, password)
        }
    };
    reader
        .get_mut()
        .write_all(format!("{prefix}{command}\r").as_bytes())
        .map_err(|e| format!("couldn't send: {e}"))?;
    let line = read_cr_line(&mut reader).map_err(|e| format!("no answer: {e}"))?;
    parse_reply(&command, &line)
}

/// Run a projector command: connect, talk, hang up.
pub fn send(out: &ProjectorOut) -> Result<(), String> {
    let host = out.host.trim();
    let name = format!("projector {host}");
    let addr = (host, out.port)
        .to_socket_addrs()
        .map_err(|e| format!("{name}: can't find it: {e}"))?
        .next()
        .ok_or_else(|| format!("{name}: no address"))?;
    let stream = TcpStream::connect_timeout(&addr, TIMEOUT)
        .map_err(|e| format!("{name}: can't connect: {e}"))?;
    stream.set_read_timeout(Some(TIMEOUT)).ok();
    stream.set_write_timeout(Some(TIMEOUT)).ok();
    converse(stream, &out.password, out.action)
        .map_err(|e| format!("{name} ({}): {e}", out.action.label()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn commands_are_the_pjlink_ones() {
        assert_eq!(command_text(ProjectorAction::PowerOn), "%1POWR 1");
        assert_eq!(command_text(ProjectorAction::PowerOff), "%1POWR 0");
        assert_eq!(command_text(ProjectorAction::ShutterClosed), "%1AVMT 11");
        assert_eq!(command_text(ProjectorAction::ShutterOpen), "%1AVMT 10");
        assert_eq!(command_text(ProjectorAction::Input(31)), "%1INPT 31");
    }

    #[test]
    fn the_digest_is_the_one_in_the_pjlink_specification() {
        // The example in the PJLink specification: random "498e4a67",
        // password "JBMIAProjectorLink".
        assert_eq!(
            digest("498e4a67", "JBMIAProjectorLink"),
            "5d8409bc1c3fa39749434aa3a5c38682"
        );
    }

    #[test]
    fn greetings_and_replies_are_read() {
        assert_eq!(parse_greeting("PJLINK 0\r"), Ok(Greeting::Open));
        assert_eq!(
            parse_greeting("PJLINK 1 498e4a67\r"),
            Ok(Greeting::Password("498e4a67".into()))
        );
        assert!(parse_greeting("PJLINK ERRA\r").is_err());
        assert!(parse_greeting("220 ftp ready").is_err());
        assert_eq!(parse_reply("%1POWR 1", "%1POWR=OK\r"), Ok(()));
        assert!(
            parse_reply("%1POWR 1", "%1POWR=ERR3\r")
                .unwrap_err()
                .contains("warming up")
        );
        assert!(parse_reply("%1POWR 1", "%1INPT=OK\r").is_err());
        assert!(
            parse_reply("%1POWR 1", "PJLINK ERRA\r")
                .unwrap_err()
                .contains("password")
        );
    }

    /// A projector on loopback: greets, then answers one command with `reply`.
    /// Returns the address and what it was sent.
    fn fake_projector(
        greeting: &'static str,
        reply: &'static str,
    ) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(greeting.as_bytes()).unwrap();
            let line = read_cr_line(&mut BufReader::new(s.try_clone().unwrap())).unwrap();
            s.write_all(reply.as_bytes()).unwrap();
            line
        });
        (port, handle)
    }

    fn out(port: u16, password: &str, action: ProjectorAction) -> ProjectorOut {
        ProjectorOut {
            host: "127.0.0.1".into(),
            port,
            password: password.into(),
            action,
        }
    }

    #[test]
    fn a_projector_without_a_password_is_told_the_command() {
        let (port, got) = fake_projector("PJLINK 0\r", "%1POWR=OK\r");
        send(&out(port, "", ProjectorAction::PowerOn)).unwrap();
        assert_eq!(got.join().unwrap(), "%1POWR 1\r");
    }

    #[test]
    fn a_projector_with_a_password_gets_the_digest_first() {
        let (port, got) = fake_projector("PJLINK 1 498e4a67\r", "%1AVMT=OK\r");
        send(&out(
            port,
            "JBMIAProjectorLink",
            ProjectorAction::ShutterClosed,
        ))
        .unwrap();
        assert_eq!(
            got.join().unwrap(),
            "5d8409bc1c3fa39749434aa3a5c38682%1AVMT 11\r"
        );
    }

    #[test]
    fn refusals_are_reported_with_the_projector_and_the_action() {
        let (port, got) = fake_projector("PJLINK 0\r", "%1POWR=ERR3\r");
        let e = send(&out(port, "", ProjectorAction::PowerOff)).unwrap_err();
        got.join().unwrap();
        assert!(
            e.contains("127.0.0.1") && e.contains("Power off") && e.contains("cooling"),
            "{e}"
        );

        // A password it needs and we don't have is said before anything is sent.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"PJLINK 1 abcd1234\r").unwrap();
            let mut rest = Vec::new();
            s.read_to_end(&mut rest).unwrap();
            rest
        });
        let e = send(&out(port, "", ProjectorAction::PowerOn)).unwrap_err();
        assert!(e.contains("password"), "{e}");
        assert!(
            h.join().unwrap().is_empty(),
            "nothing should have been sent"
        );
    }

    #[test]
    fn nobody_home_is_an_error_not_a_hang() {
        // A port nothing listens on.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let e = send(&out(port, "", ProjectorAction::PowerOn)).unwrap_err();
        assert!(e.contains("can't connect"), "{e}");
    }
}
