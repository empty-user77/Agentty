//! Who may use the remote page, past Tailscale's own check: the web password, the sessions it
//! opens and the lockout after wrong guesses. No I/O here, so every rule is tested directly.
//!
//! - The password is stored only as a PBKDF2-HMAC-SHA256 hash with a random salt
//!   (`pbkdf2-sha256$<iterations>$<salt>$<hash>`, hex) and checked in constant time.
//! - A session is a random 256-bit token in an HttpOnly cookie. Only its SHA-256 is kept, so the
//!   table in memory can't be replayed; it is tied to the Tailscale login that opened it, expires
//!   when idle and after a fixed time, and every session ends with the app, a password change or
//!   "Sign out everywhere".
//! - Five wrong passwords lock the page for a minute; every wrong password after a lock locks it
//!   again for twice as long, up to an hour. One right password resets it.

use ring::digest;
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

/// Clock time, not `SystemTime`: on macOS an `SystemTime` stops while the Mac sleeps, which would stretch
/// every expiry and lock by however long it slept. A clock set back counts as no time passed.
fn since(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or(Duration::ZERO)
}

pub const MIN_PASSWORD_CHARS: usize = 8;
/// Longer passwords are refused rather than hashed: PBKDF2's cost grows with them.
pub const MAX_PASSWORD_BYTES: usize = 256;
/// OWASP's figure for PBKDF2-HMAC-SHA256.
const ITERATIONS: u32 = 600_000;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
const SCHEME: &str = "pbkdf2-sha256";

pub const SESSION_IDLE: Duration = Duration::from_secs(12 * 60 * 60);
pub const SESSION_MAX: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// More devices than anyone uses; the oldest session makes room for a new one.
pub const MAX_SESSIONS: usize = 8;

const FREE_TRIES: u32 = 5;
const LOCK_BASE: Duration = Duration::from_secs(60);
const LOCK_MAX: Duration = Duration::from_secs(60 * 60);

/// Why a new password is refused, as an i18n key: at least eight characters, with a digit and a
/// special character among them.
pub fn password_problem(password: &str) -> Option<&'static str> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        Some("remote.password_short")
    } else if password.len() > MAX_PASSWORD_BYTES {
        Some("remote.password_long")
    } else if !password.chars().all(|c| c.is_ascii_graphic()) {
        // Typed with a Korean (or other) input method on, a masked field stores jamo where the
        // user meant Latin letters, and the browser's password field, which turns input methods
        // off, can then never match it. Only what a password field anywhere types is accepted.
        Some("remote.password_ascii")
    } else if !password.chars().any(|c| c.is_ascii_digit()) {
        Some("remote.password_needs_digit")
    } else if !password.chars().any(|c| !c.is_alphanumeric() && !c.is_whitespace()) {
        Some("remote.password_needs_symbol")
    } else {
        None
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    SystemRandom::new().fill(&mut out).expect("the system random generator failed");
    out
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// The stored form of `password`.
pub fn hash_password(password: &str) -> String {
    hash_with(password, ITERATIONS)
}

/// A cheap hash for tests elsewhere, which would otherwise spend most of their time in PBKDF2.
#[cfg(test)]
pub fn hash_for_tests(password: &str) -> String {
    hash_with(password, 1_000)
}

fn hash_with(password: &str, iterations: u32) -> String {
    let salt = random_bytes::<SALT_LEN>();
    let mut hash = [0u8; HASH_LEN];
    let rounds = NonZeroU32::new(iterations).expect("iterations > 0");
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, rounds, &salt, password.as_bytes(), &mut hash);
    format!("{SCHEME}${iterations}${}${}", to_hex(&salt), to_hex(&hash))
}

/// Whether `password` is the one `stored` was made from. Constant time in the password; a stored
/// value in any other shape never matches.
pub fn verify_password(stored: &str, password: &str) -> bool {
    if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
        return false;
    }
    let mut parts = stored.split('$');
    let (Some(SCHEME), Some(iterations), Some(salt), Some(hash), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Some(rounds) = iterations.parse::<u32>().ok().filter(|n| (1_000..=10_000_000).contains(n)).and_then(NonZeroU32::new) else {
        return false;
    };
    let (Some(salt), Some(hash)) = (from_hex(salt), from_hex(hash)) else { return false };
    if salt.len() < 8 || hash.len() != HASH_LEN {
        return false;
    }
    pbkdf2::verify(pbkdf2::PBKDF2_HMAC_SHA256, rounds, &salt, password.as_bytes(), &hash).is_ok()
}

/// A fresh session token: 32 random bytes, hex.
pub fn new_token() -> String {
    to_hex(&random_bytes::<32>())
}

fn token_key(token: &str) -> [u8; 32] {
    let digest = digest::digest(&digest::SHA256, token.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(digest.as_ref());
    key
}

/// Looks like a token this module made (64 hex digits): anything else is refused before hashing.
fn token_shape(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Debug, Clone)]
pub struct Session {
    pub created: SystemTime,
    pub last_seen: SystemTime,
    /// The Tailscale login that opened it; a request from any other login can't use it.
    pub login: String,
    /// A short description of the browser, for the settings page.
    pub device: String,
}

#[derive(Debug, Default)]
pub struct Sessions {
    by_key: HashMap<[u8; 32], Session>,
}

impl Sessions {
    /// Opens a session for `login` and returns its token (shown once, in the cookie).
    pub fn create(&mut self, login: &str, device: &str, now: SystemTime) -> String {
        self.prune(now);
        while self.by_key.len() >= MAX_SESSIONS {
            let Some(oldest) = self.by_key.iter().min_by_key(|(_, s)| s.last_seen).map(|(k, _)| *k) else { break };
            self.by_key.remove(&oldest);
        }
        let token = new_token();
        let device: String = device.chars().filter(|c| !c.is_control()).take(80).collect();
        self.by_key.insert(token_key(&token), Session { created: now, last_seen: now, login: login.to_string(), device });
        token
    }

    /// Whether `token` is a live session of `login`; a live one counts as used now.
    pub fn check(&mut self, token: &str, login: &str, now: SystemTime) -> bool {
        if !token_shape(token) {
            return false;
        }
        let key = token_key(token);
        let Some(session) = self.by_key.get_mut(&key) else { return false };
        let expired = since(now, session.last_seen) > SESSION_IDLE || since(now, session.created) > SESSION_MAX;
        if expired {
            self.by_key.remove(&key);
            return false;
        }
        if session.login != login {
            return false;
        }
        session.last_seen = now;
        true
    }

    pub fn remove(&mut self, token: &str) {
        if token_shape(token) {
            self.by_key.remove(&token_key(token));
        }
    }

    pub fn clear(&mut self) {
        self.by_key.clear();
    }

    fn prune(&mut self, now: SystemTime) {
        self.by_key.retain(|_, s| since(now, s.last_seen) <= SESSION_IDLE && since(now, s.created) <= SESSION_MAX);
    }

    /// The live sessions, newest use first, for the settings page.
    pub fn list(&mut self, now: SystemTime) -> Vec<Session> {
        self.prune(now);
        let mut sessions: Vec<Session> = self.by_key.values().cloned().collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.last_seen));
        sessions
    }
}

/// Wrong-password lockout. One counter for the whole page: every request reaches Agentty through
/// Tailscale's proxy from the same address, and the password is the same for every device.
#[derive(Debug, Default)]
pub struct Lockout {
    failures: u32,
    /// How many locks in a row so far: each one lasts twice the one before.
    locks: u32,
    locked_until: Option<SystemTime>,
}

impl Lockout {
    /// How long until a password may be tried again, when locked.
    pub fn remaining(&self, now: SystemTime) -> Option<Duration> {
        self.locked_until.and_then(|until| until.duration_since(now).ok()).filter(|left| !left.is_zero())
    }

    /// A wrong password. Returns the lock it started, if it started one.
    pub fn fail(&mut self, now: SystemTime) -> Option<Duration> {
        self.failures += 1;
        if self.locks == 0 && self.failures < FREE_TRIES {
            return None;
        }
        let factor = 1u32.checked_shl(self.locks).unwrap_or(u32::MAX);
        let lock = LOCK_BASE.checked_mul(factor).unwrap_or(LOCK_MAX).min(LOCK_MAX);
        self.locks = self.locks.saturating_add(1);
        self.failures = 0;
        self.locked_until = Some(now + lock);
        Some(lock)
    }

    pub fn succeed(&mut self) {
        *self = Lockout::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_check_and_reject() {
        let stored = hash_with("correct horse battery", 1_000);
        assert!(stored.starts_with("pbkdf2-sha256$1000$"));
        assert!(verify_password(&stored, "correct horse battery"));
        assert!(!verify_password(&stored, "correct horse batterY"));
        assert!(!verify_password(&stored, ""));
        assert_ne!(stored, hash_with("correct horse battery", 1_000), "a fresh salt every time");
    }

    #[test]
    fn malformed_stored_values_never_match() {
        let good = hash_with("password123", 1_000);
        let parts: Vec<&str> = good.split('$').collect();
        for bad in [
            String::new(),
            "plain-text-password".to_string(),
            format!("md5${}${}${}", parts[1], parts[2], parts[3]),
            format!("{}$0${}${}", parts[0], parts[2], parts[3]),
            format!("{}$999999999${}${}", parts[0], parts[2], parts[3]),
            format!("{}${}$zz${}", parts[0], parts[1], parts[3]),
            format!("{}${}${}$abcd", parts[0], parts[1], parts[2]),
            format!("{good}$extra"),
        ] {
            assert!(!verify_password(&bad, "password123"), "{bad}");
        }
    }

    #[test]
    fn password_rules() {
        assert_eq!(password_problem("sh0rt!"), Some("remote.password_short"));
        assert_eq!(password_problem("longenough!"), Some("remote.password_needs_digit"));
        assert_eq!(password_problem("longenough1"), Some("remote.password_needs_symbol"));
        assert_eq!(password_problem("long enough 1!"), Some("remote.password_ascii"), "no spaces");
        assert_eq!(password_problem("abcdefg1!"), None);
        assert_eq!(password_problem("ㅅㄷㄴㅅㅔㅁㄴㄴ1!"), Some("remote.password_ascii"), "typed with the Korean input method on");
        assert_eq!(password_problem("비밀번호여덟1!"), Some("remote.password_ascii"));
        assert_eq!(password_problem(&"x".repeat(MAX_PASSWORD_BYTES + 1)), Some("remote.password_long"));
        assert!(!verify_password(&hash_with("abc", 1_000), &"x".repeat(MAX_PASSWORD_BYTES + 1)));
    }

    #[test]
    fn sessions_belong_to_their_login_and_expire() {
        let start = SystemTime::now();
        let mut sessions = Sessions::default();
        let token = sessions.create("me@example.com", "Safari", start);
        assert_eq!(token.len(), 64);
        assert!(sessions.check(&token, "me@example.com", start));
        assert!(!sessions.check(&token, "someone@example.com", start), "another login can't use it");
        assert!(!sessions.check(&new_token(), "me@example.com", start), "unknown token");
        assert!(!sessions.check("not-hex", "me@example.com", start));
        assert!(sessions.check(&token, "me@example.com", start + SESSION_IDLE - Duration::from_secs(1)));
        let later = start + SESSION_IDLE - Duration::from_secs(1) + SESSION_IDLE + Duration::from_secs(1);
        assert!(!sessions.check(&token, "me@example.com", later), "idle too long");
        assert!(!sessions.check(&token, "me@example.com", start), "and gone for good");
    }

    #[test]
    fn sessions_end_at_their_maximum_age_even_when_used() {
        let start = SystemTime::now();
        let mut sessions = Sessions::default();
        let token = sessions.create("me", "x", start);
        let mut now = start;
        while now < start + SESSION_MAX - Duration::from_secs(3600) {
            now += Duration::from_secs(3600);
            assert!(sessions.check(&token, "me", now));
        }
        assert!(!sessions.check(&token, "me", start + SESSION_MAX + Duration::from_secs(1)));
    }

    #[test]
    fn sessions_are_capped_and_cleared() {
        let start = SystemTime::now();
        let mut sessions = Sessions::default();
        let first = sessions.create("me", "a", start);
        for i in 1..=MAX_SESSIONS {
            sessions.create("me", "b", start + Duration::from_secs(i as u64));
        }
        assert!(!sessions.check(&first, "me", start + Duration::from_secs(60)), "the oldest made room");
        assert_eq!(sessions.list(start + Duration::from_secs(60)).len(), MAX_SESSIONS);
        let token = sessions.create("me", "c", start + Duration::from_secs(61));
        sessions.remove(&token);
        assert!(!sessions.check(&token, "me", start + Duration::from_secs(62)));
        sessions.clear();
        assert!(sessions.list(start).is_empty());
    }

    #[test]
    fn lockout_grows_and_resets() {
        let start = SystemTime::now();
        let mut lock = Lockout::default();
        for _ in 0..4 {
            assert_eq!(lock.fail(start), None);
        }
        assert_eq!(lock.remaining(start), None);
        assert_eq!(lock.fail(start), Some(LOCK_BASE), "fifth wrong password");
        assert_eq!(lock.remaining(start), Some(LOCK_BASE));
        let after = start + LOCK_BASE + Duration::from_secs(1);
        assert_eq!(lock.remaining(after), None);
        assert_eq!(lock.fail(after), Some(LOCK_BASE * 2), "one more after a lock locks again, longer");
        let mut now = after;
        for _ in 0..10 {
            now += LOCK_MAX + Duration::from_secs(1);
            assert!(lock.fail(now).unwrap() <= LOCK_MAX);
        }
        lock.succeed();
        assert_eq!(lock.remaining(now), None);
        assert_eq!(lock.fail(now), None, "a right password gives the free tries back");
    }
}
