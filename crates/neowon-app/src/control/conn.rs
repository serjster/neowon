//! One control-socket connection: the accept loop, the line framing, and
//! the token handshake that decides what the connection may run.
//!
//! The socket is on by default and binds 127.0.0.1, so *any* local process
//! — including a web page that POSTs to `http://127.0.0.1:7777/`, whose
//! header and body lines arrive here as script lines — can reach it.
//! Reading instrument state and driving the instrument is what the socket
//! is for and stays open; writing files, reading files the caller names,
//! reaching the network and ending the process do not, and need a token
//! (`script::privilege`).
//!
//! **How a legitimate client gets the token**, in the order they try:
//!
//! - `NEOWON_CONTROL_TOKEN=<value>`: a parent that spawns the app picks the
//!   token itself. The test harness (`tests/common/mod.rs`) does this.
//! - The token file: with no environment token the app generates one and
//!   writes it to `$HOME/.neowon/control/<port>.token`, mode 0600, so a
//!   client running as the operator can read it and nothing else can. A
//!   client that does not know where to look sends a bare `auth` and is
//!   told the path (`neowon-mcp` does exactly this); the operator's `nc`
//!   loop does `auth $(cat …)`.
//!
//! The token is never logged and never returned by a query — only the
//! path of the file holding it, which a caller that cannot read the file
//! cannot use.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::{Request, escape, orphan};
use crate::script::{Action, parse};

/// Bad `auth` attempts before the connection is dropped. A wrong token is
/// a mistake or a guess; neither needs a fourth try on one connection.
const MAX_AUTH_FAILURES: u32 = 3;

/// The process's control token and where a legitimate client finds it.
pub struct Auth {
    token: String,
    /// The 0600 file holding the token, when the app generated and wrote
    /// one. `None` when the token came from the environment (the parent
    /// already has it) or no file could be written.
    file: Option<PathBuf>,
}

impl Auth {
    /// The token for this process: `NEOWON_CONTROL_TOKEN` when set,
    /// otherwise a fresh random one written to the per-port token file.
    #[must_use]
    pub fn for_port(port: u16) -> Arc<Self> {
        if let Ok(t) = std::env::var("NEOWON_CONTROL_TOKEN")
            && !t.trim().is_empty()
        {
            return Arc::new(Self {
                token: t.trim().to_string(),
                file: None,
            });
        }
        let token = random_token();
        let file = token_path(port).filter(|p| match write_token(p, &token) {
            Ok(()) => {
                info!("control: token in {}", p.display());
                true
            }
            Err(e) => {
                error!("control: cannot write {}: {e}", p.display());
                false
            }
        });
        if file.is_none() {
            // Without a file the token would be undiscoverable and the
            // socket read-only. Printing it is the lesser evil, and only
            // this process's own console sees it.
            eprintln!("control: no token file; write verbs need `auth {token}`");
        }
        Arc::new(Self { token, file })
    }

    /// Constant-ish comparison: same length and no early return, so a
    /// caller cannot time its way to the token byte by byte.
    fn matches(&self, offered: &str) -> bool {
        let (a, b) = (self.token.as_bytes(), offered.trim().as_bytes());
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
    }

    /// The refusal a client gets when it needs the token, naming where to
    /// find it (never the token itself).
    #[must_use]
    pub fn challenge(&self, reason: &str) -> String {
        match &self.file {
            Some(p) => format!(
                r#"{{"ok":false,"error":"{}","auth":"token","token_file":"{}"}}"#,
                escape(reason),
                escape(&p.display().to_string())
            ),
            None => format!(
                r#"{{"ok":false,"error":"{}","auth":"token","token_env":"NEOWON_CONTROL_TOKEN"}}"#,
                escape(reason)
            ),
        }
    }
}

/// `$HOME/.neowon/control/<port>.token`, or `None` with no home directory.
fn token_path(port: u16) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".neowon/control")
            .join(format!("{port}.token")),
    )
}

/// Write the token so only the operator can read it. A stale file left by
/// a killed app is harmless: nothing listens on that port, and the next
/// launch overwrites it.
fn write_token(path: &std::path::Path, token: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    // Owner-only from its first byte, and whole when it appears: a client
    // polling for the token never reads a truncated one.
    let mut f = neowon_core::atomic_file::AtomicFile::create_private(path)?;
    writeln!(f, "{token}")?;
    f.commit()
}

/// 128 unpredictable bits, hex. The OS pool is the source where it can be
/// read directly; `RandomState`'s keys — which std seeds from the same OS
/// pool once per process — are the fallback where it cannot (Windows).
/// No dependency is added for this (AGENTS.md); if that fallback ever has
/// to carry more than a localhost token, `getrandom` is the right fix and
/// is an operator decision.
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if read_os_random(&mut bytes).is_none() {
        fill_from_hasher_keys(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_os_random(out: &mut [u8; 16]) -> Option<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    f.read_exact(out).ok()
}

fn fill_from_hasher_keys(out: &mut [u8; 16]) {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mix = |salt: u64| -> u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u32(std::process::id());
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        h.write_usize(Box::into_raw(Box::new(0u8)) as usize);
        h.finish()
    };
    out[..8].copy_from_slice(&mix(0).to_le_bytes());
    out[8..].copy_from_slice(&mix(1).to_le_bytes());
}

/// What one control line means for a connection with this trust: the
/// actions to inject, or the reply to send back instead.
///
/// `get …` queries never reach here — they are read-only and answered in
/// `control::poll`.
pub fn authorize(line: &str, authed: bool, auth: &Auth) -> Result<Vec<(f64, Action)>, String> {
    let actions = parse(line).map_err(|e| format!(r#"{{"ok":false,"error":"{}"}}"#, escape(&e)))?;
    if !authed && let Some((_, a)) = actions.iter().find(|(_, a)| a.privilege().needs_token()) {
        let verb = line.split_whitespace().next().unwrap_or(line);
        debug_assert!(a.privilege().needs_token());
        return Err(auth.challenge(&format!(
            "{verb} writes outside the app; send `auth <token>` on this connection first"
        )));
    }
    Ok(actions.into())
}

/// Accept connections forever, forwarding each line as a [`Request`].
pub fn accept_loop(
    listener: TcpListener,
    tx: Sender<Request>,
    guard: Option<Arc<orphan::OrphanGuard>>,
    auth: Arc<Auth>,
) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let (tx, guard, auth) = (tx.clone(), guard.clone(), Arc::clone(&auth));
        std::thread::spawn(move || {
            // Live for as long as the connection is: dropping the token
            // on any exit path restarts the orphan watchdog's clock.
            let _live = guard.as_ref().map(|g| g.connect());
            serve(conn, &tx, &auth);
        });
    }
}

/// One connection's read loop. Trust is per-connection and starts at
/// none: an `auth` on one socket never lifts another's.
fn serve(conn: TcpStream, tx: &Sender<Request>, auth: &Auth) {
    let Ok(mut out) = conn.try_clone() else {
        return;
    };
    let mut authed = false;
    let mut failures = 0u32;
    for line in BufReader::new(conn).lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        // The handshake is answered here, not in the app: it is about this
        // socket, and it must work before the app has finished starting.
        // Split on the verb, not on a prefix: `strip_prefix("auth")` would
        // also swallow a future verb whose name starts with it.
        let (verb, arg) = line.split_once(char::is_whitespace).unwrap_or((&line, ""));
        let reply = if verb == "auth" {
            let offered = arg.trim();
            if offered.is_empty() {
                auth.challenge("token required for write verbs")
            } else if auth.matches(offered) {
                authed = true;
                r#"{"ok":true,"authed":true}"#.into()
            } else {
                failures += 1;
                r#"{"ok":false,"error":"bad token"}"#.into()
            }
        } else {
            let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
            if tx
                .send(Request {
                    line,
                    authed,
                    reply: reply_tx,
                })
                .is_err()
            {
                break;
            }
            reply_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap_or_else(|_| r#"{"ok":false,"error":"timeout"}"#.into())
        };
        if writeln!(out, "{reply}").is_err() {
            break;
        }
        if failures >= MAX_AUTH_FAILURES {
            let _ = writeln!(out, r#"{{"ok":false,"error":"too many auth failures"}}"#);
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Auth, authorize, random_token};
    use std::sync::Arc;

    fn fixed() -> Arc<Auth> {
        Arc::new(Auth {
            token: "s3cret".into(),
            file: Some("/home/op/.neowon/control/7777.token".into()),
        })
    }

    #[test]
    fn a_token_is_unpredictable_and_hex() {
        let a = random_token();
        assert_eq!(a.len(), 32, "{a}");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(a, random_token(), "two tokens must differ");
    }

    #[test]
    fn only_the_exact_token_matches() {
        let auth = fixed();
        assert!(auth.matches("s3cret"));
        assert!(auth.matches(" s3cret "));
        assert!(!auth.matches("s3cre"));
        assert!(!auth.matches("s3crett"));
        assert!(!auth.matches("S3cret"));
        assert!(!auth.matches(""));
    }

    /// The refusal points at the token file and never carries the token.
    #[test]
    fn the_challenge_names_the_file_not_the_token() {
        let auth = fixed();
        let c = auth.challenge("nope");
        assert!(c.contains(r#""ok":false"#), "{c}");
        assert!(c.contains("7777.token"), "{c}");
        assert!(!c.contains("s3cret"), "{c}");
    }

    /// The invariant: an unauthenticated connection cannot run a verb that
    /// leaves the process; the same line from an authenticated one does.
    #[test]
    fn an_unauthenticated_connection_cannot_run_a_write_verb() {
        let auth = fixed();
        for line in [
            "shot /tmp/a.png",
            "shotplot /tmp/a.ppm",
            "sdr iqdump /tmp/a.f32 1",
            "quit",
            "sessionload /etc/passwd",
            "capload /etc/passwd",
            "export csv /tmp/a.csv",
            "refdb fetch eibi",
            "catalog delete 1",
        ] {
            let refused = authorize(line, false, &auth).expect_err(line);
            assert!(refused.contains(r#""ok":false"#), "{line}: {refused}");
            assert!(refused.contains("token_file"), "{line}: {refused}");
            assert!(
                authorize(line, true, &auth).is_ok(),
                "{line} must run once authenticated"
            );
        }
    }

    /// Driving the instrument needs no token, authenticated or not — the
    /// `nc` loop in AGENTS.md keeps working untouched.
    #[test]
    fn live_state_lines_run_unauthenticated() {
        let auth = fixed();
        for line in [
            "run 1",
            "sdr tune 100M",
            "vdiv 0 0.05",
            "stimulus xy-circle",
        ] {
            assert!(authorize(line, false, &auth).is_ok(), "{line}");
        }
    }

    /// A parse error is still a parse error, not an auth challenge.
    #[test]
    fn an_unknown_verb_is_refused_as_a_parse_error() {
        let auth = fixed();
        let e = authorize("florp 1", false, &auth).expect_err("florp");
        assert!(e.contains("unknown action"), "{e}");
        assert!(!e.contains("token_file"), "{e}");
    }
}
