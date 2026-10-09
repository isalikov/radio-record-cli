//! Account state and background sync. Local favorites are never replaced until
//! the chosen server update has been verified; the baseline belongs to a user.
use crate::{
    api::{Client, Profile},
    app::App,
    error::{Error, Result},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub device_code: String,
    pub user_id: i64,
    pub baseline: Option<BTreeSet<i64>>,
}

impl Session {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| Error::new("Invalid saved account · sign in again")),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Error::new("Could not read saved account · sign in again")),
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::new("Missing account directory"))?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        // NamedTempFile is private by default; enforce it explicitly on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer_pretty(&mut file, self)
            .map_err(|_| Error::new("Could not encode account session"))?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(path)
            .map_err(|_| Error::new("Could not save account session"))?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Merge,
    Local,
    Server,
}

pub struct Conflict {
    pub remote: BTreeSet<i64>,
    pub choice: Choice,
}

/// With a baseline, preserve independent additions and honor removals from
/// either side. On first login there is no removal history: merge is a union.
pub fn merge(
    base: Option<&BTreeSet<i64>>,
    local: &BTreeSet<i64>,
    remote: &BTreeSet<i64>,
) -> BTreeSet<i64> {
    let Some(base) = base else {
        return local.union(remote).copied().collect();
    };
    local
        .union(remote)
        .copied()
        .filter(|id| !base.contains(id) || (local.contains(id) && remote.contains(id)))
        .collect()
}

#[derive(Default)]
pub struct Account {
    pub open: bool,
    pub email: String,
    pub password: String,
    pub password_focus: bool,
    pub profile: Option<Profile>,
    pub session: Option<Session>,
    pub busy: bool,
    pub error: Option<String>,
    pub message: Option<String>,
    pub conflict: Option<Conflict>,
    pub logout_confirm: bool,
    pub last_sync: Option<Instant>,
}

pub enum Command {
    Login { email: String, password: String },
    Sync,
    Resolve(Choice),
    Logout,
}

impl Account {
    pub fn key(&mut self, key: KeyEvent) -> Option<Command> {
        if key.code == KeyCode::Esc {
            self.open = false;
            self.password.clear();
            self.logout_confirm = false;
            return None;
        }
        if self.logout_confirm {
            return match key.code {
                KeyCode::Enter => Some(Command::Logout),
                _ => {
                    self.logout_confirm = false;
                    None
                }
            };
        }
        if self.session.is_some() && key.code == KeyCode::Char('l') {
            self.logout_confirm = true;
            return None;
        }
        if self.busy {
            return None;
        }
        if let Some(conflict) = &mut self.conflict {
            let choices = [Choice::Merge, Choice::Local, Choice::Server];
            let selected = choices
                .iter()
                .position(|c| *c == conflict.choice)
                .unwrap_or(0);
            match key.code {
                KeyCode::Up | KeyCode::BackTab => conflict.choice = choices[(selected + 2) % 3],
                KeyCode::Down | KeyCode::Tab => conflict.choice = choices[(selected + 1) % 3],
                KeyCode::Char('1') => conflict.choice = Choice::Merge,
                KeyCode::Char('2') => conflict.choice = Choice::Local,
                KeyCode::Char('3') => conflict.choice = Choice::Server,
                KeyCode::Enter => return Some(Command::Resolve(conflict.choice)),
                KeyCode::Char('l') => self.logout_confirm = true,
                _ => {}
            }
            return None;
        }
        if self.session.is_some() {
            match key.code {
                KeyCode::Char('s') | KeyCode::Enter => return Some(Command::Sync),
                KeyCode::Char('l') => self.logout_confirm = true,
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                self.password_focus = !self.password_focus
            }
            KeyCode::Enter if !self.password_focus => self.password_focus = true,
            KeyCode::Enter => {
                let email = self.email.trim().to_owned();
                if email.is_empty() || self.password.is_empty() {
                    self.error = Some("Enter your email and password".into());
                } else {
                    return Some(Command::Login {
                        email,
                        password: std::mem::take(&mut self.password),
                    });
                }
            }
            KeyCode::Backspace => {
                self.input().pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input().clear()
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let input = self.input();
                if input.len() < 1024 && !ch.is_control() {
                    input.push(ch);
                }
            }
            _ => {}
        }
        None
    }

    fn input(&mut self) -> &mut String {
        if self.password_focus {
            &mut self.password
        } else {
            &mut self.email
        }
    }

    pub fn paste(&mut self, text: &str) {
        if !self.open || self.busy || self.session.is_some() || self.conflict.is_some() {
            return;
        }
        for ch in text.chars().filter(|ch| !ch.is_control()) {
            let input = self.input();
            if input.len() + ch.len_utf8() > 1024 {
                break;
            }
            input.push(ch);
        }
    }
}

enum Reply {
    LoggedIn {
        session: Session,
        profile: Profile,
    },
    Fetched {
        session: Session,
        profile: Profile,
        remote: BTreeSet<i64>,
    },
    Applied {
        session: Session,
        before: BTreeSet<i64>,
        target: BTreeSet<i64>,
    },
}

pub struct Network {
    path: PathBuf,
    client: Client,
    pending: Option<Receiver<Result<Reply>>>,
    cancelled: Arc<AtomicBool>,
    checked: Instant,
    observed: BTreeSet<i64>,
    changed: Instant,
}

impl Network {
    pub fn new(settings_path: &Path) -> Self {
        Self {
            path: settings_path.with_file_name("account.json"),
            client: Client::new(),
            pending: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            checked: Instant::now(),
            observed: BTreeSet::new(),
            changed: Instant::now(),
        }
    }

    pub fn restore(&mut self, app: &mut App) {
        self.observed = app.settings.favorites.clone();
        match Session::load(&self.path) {
            Ok(session) => {
                app.account.session = session;
                if app.account.session.is_some() {
                    self.command(Command::Sync, app);
                }
            }
            Err(err) => {
                app.account.error = Some(err.to_string());
            }
        }
    }

    fn spawn(&mut self, app: &mut App, job: impl FnOnce() -> Result<Reply> + Send + 'static) {
        app.account.busy = true;
        app.account.error = None;
        app.account.message = None;
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        thread::spawn(move || {
            let _ = tx.send(job());
        });
    }

    pub fn command(&mut self, command: Command, app: &mut App) {
        if matches!(command, Command::Logout) {
            self.cancelled.store(true, Ordering::Release);
            self.pending = None;
            if let Err(err) = fs::remove_file(&self.path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                app.account.busy = false;
                app.account.logout_confirm = false;
                app.account.error =
                    Some("Could not remove saved session · l retry sign out".into());
                self.cancelled = Arc::new(AtomicBool::new(false));
                return;
            }
            let email = app
                .account
                .profile
                .as_ref()
                .map(|p| p.email.clone())
                .unwrap_or_default();
            app.account = Account {
                open: true,
                email,
                ..Account::default()
            };
            self.cancelled = Arc::new(AtomicBool::new(false));
            return;
        }
        if self.pending.is_some() {
            return;
        }
        match command {
            Command::Login { email, password } => {
                let client = self.client.clone();
                self.spawn(app, move || {
                    let login = client.login(&email, &password)?;
                    // Keep a successful login even if the favorites request fails:
                    // favorites are fetched separately once the session reaches UI.
                    Ok(Reply::LoggedIn {
                        session: Session {
                            device_code: login.device_code,
                            user_id: login.profile.id,
                            baseline: None,
                        },
                        profile: login.profile,
                    })
                });
            }
            Command::Sync => {
                let Some(session) = app.account.session.clone() else {
                    return;
                };
                app.account.conflict = None;
                let client = self.client.clone();
                self.spawn(app, move || {
                    let profile = client.get_profile(&session.device_code)?;
                    if profile.id != session.user_id {
                        return Err(Error::new("Account changed · sign in again"));
                    }
                    let remote = client.get_favorites(&session.device_code)?;
                    Ok(Reply::Fetched {
                        session,
                        profile,
                        remote,
                    })
                });
            }
            Command::Resolve(choice) => {
                let Some(conflict) = app.account.conflict.take() else {
                    return;
                };
                let target = match choice {
                    Choice::Merge => merge(
                        app.account
                            .session
                            .as_ref()
                            .and_then(|s| s.baseline.as_ref()),
                        &app.settings.favorites,
                        &conflict.remote,
                    ),
                    Choice::Local => app.settings.favorites.clone(),
                    Choice::Server => conflict.remote.clone(),
                };
                self.apply(app, conflict.remote, target);
            }
            Command::Logout => unreachable!(),
        }
    }

    fn apply(&mut self, app: &mut App, expected: BTreeSet<i64>, target: BTreeSet<i64>) {
        let Some(mut session) = app.account.session.clone() else {
            return;
        };
        let before = app.settings.favorites.clone();
        let cancel = self.cancelled.clone();
        let client = self.client.clone();
        self.spawn(app, move || {
            let remote = client.get_favorites(&session.device_code)?;
            if remote != expected {
                return Ok(Reply::Fetched {
                    profile: client.get_profile(&session.device_code)?,
                    session,
                    remote,
                });
            }
            for (ids, add) in [
                (target.difference(&remote), true),
                (remote.difference(&target), false),
            ] {
                for id in ids {
                    if cancel.load(Ordering::Acquire) {
                        return Err(Error::new("Sync cancelled"));
                    }
                    client.set_favorite(&session.device_code, *id, add)?;
                }
            }
            if cancel.load(Ordering::Acquire) {
                return Err(Error::new("Sync cancelled"));
            }
            let verified = client.get_favorites(&session.device_code)?;
            if verified != target {
                return Err(Error::new("Server changed during sync · s retry"));
            }
            session.baseline = Some(target.clone());
            Ok(Reply::Applied {
                session,
                before,
                target,
            })
        });
    }

    pub fn poll(&mut self, app: &mut App) {
        if self.observed != app.settings.favorites {
            self.observed = app.settings.favorites.clone();
            self.changed = Instant::now();
        }
        let Some(rx) = &self.pending else {
            let pending_changes = app.account.session.as_ref().is_some_and(|session| {
                session
                    .baseline
                    .as_ref()
                    .is_some_and(|base| *base != app.settings.favorites)
            });
            if app.account.session.is_some()
                && app.account.conflict.is_none()
                && app.account.error.is_none()
                && ((pending_changes && self.changed.elapsed() >= Duration::from_millis(750))
                    || self.checked.elapsed() >= Duration::from_secs(30))
            {
                self.command(Command::Sync, app);
            }
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(Error::new("Account request interrupted · s retry"))
            }
        };
        self.pending = None;
        app.account.busy = false;
        self.checked = Instant::now();
        match result {
            Err(err) => app.account.error = Some(err.to_string()),
            Ok(Reply::LoggedIn { session, profile }) => {
                app.account.profile = Some(profile);
                app.account.session = Some(session);
                self.save(app);
                self.command(Command::Sync, app);
            }
            Ok(Reply::Fetched {
                session,
                profile,
                remote,
            }) => {
                if profile.id != session.user_id {
                    app.account.error = Some("Account changed · sign in again".into());
                    return;
                }
                app.account.profile = Some(profile);
                app.account.session = Some(session);
                self.plan(app, remote);
            }
            Ok(Reply::Applied {
                session,
                before,
                target,
            }) => {
                // A user may toggle favorites while the worker is syncing. Keep
                // those new edits rather than overwriting with a stale snapshot.
                let local = merge(Some(&before), &app.settings.favorites, &target);
                app.replace_favorites(local);
                app.account.session = Some(session);
                app.account.last_sync = Some(Instant::now());
                app.account.message = Some("Favorites synced".into());
                self.save(app);
            }
        }
    }

    fn plan(&mut self, app: &mut App, remote: BTreeSet<i64>) {
        let local = &app.settings.favorites;
        if *local == remote {
            if let Some(session) = &mut app.account.session {
                session.baseline = Some(remote);
            }
            app.account.last_sync = Some(Instant::now());
            app.account.message = Some("Favorites synced".into());
            self.save(app);
        } else {
            let base = app
                .account
                .session
                .as_ref()
                .and_then(|s| s.baseline.as_ref());
            // A known change on one side can sync automatically. Both changed,
            // or no shared baseline: let the user review the choice first.
            if base == Some(&remote) {
                self.apply(app, remote, local.clone());
                return;
            }
            if base == Some(local) {
                self.apply(app, remote.clone(), remote);
                return;
            }
            app.account.conflict = Some(Conflict {
                remote,
                choice: Choice::Merge,
            });
            app.account.open = true;
        }
    }

    fn save(&mut self, app: &mut App) {
        // Save local favorites before their baseline. A crash between the two
        // writes must never make an old local copy look like new user edits.
        if app.dirty {
            if app
                .settings
                .save(&self.path.with_file_name("settings.json"))
                .is_err()
            {
                app.account.error = Some("Could not save favorites · s retry".into());
                return;
            }
            app.dirty = false;
        }
        if let Some(session) = &app.account.session {
            match session.save(&self.path) {
                Ok(()) => {}
                Err(_) => {
                    app.account.error = Some(
                        "Could not save session · s retry; sign-in may be lost on exit".into(),
                    );
                }
            }
        }
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::tests::{PROFILE, server},
        app::tests::{fixture, press},
    };
    use crossterm::event::KeyCode;

    fn session(base: Option<&[i64]>) -> Session {
        Session {
            device_code: "test-device-code".into(),
            user_id: 42,
            baseline: base.map(|ids| ids.iter().copied().collect()),
        }
    }

    fn pump(network: &mut Network, app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.account.busy {
            assert!(Instant::now() < deadline, "account worker did not finish");
            network.poll(app);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn merge_keeps_independent_additions_and_honors_both_sides_removals() {
        let base = BTreeSet::from([1, 2, 3]);
        let local = BTreeSet::from([1, 3, 4]);
        let remote = BTreeSet::from([1, 2, 5]);
        assert_eq!(
            merge(None, &local, &remote),
            BTreeSet::from([1, 2, 3, 4, 5])
        );
        assert_eq!(
            merge(Some(&base), &local, &remote),
            BTreeSet::from([1, 4, 5])
        );
        assert_eq!(merge(Some(&base), &local, &base), local);
        assert_eq!(merge(Some(&base), &base, &remote), remote);
    }

    #[test]
    fn login_does_not_change_local_favorites_until_conflict_is_resolved() {
        let remote = r#"{"result":{"stations":{"add":[2,3],"remove":[]}}}"#;
        let merged = r#"{"result":{"stations":{"add":[1,2,3],"remove":[]}}}"#;
        let (url, server) = server(vec![
            (
                200,
                r#"{"result":{"user":{"id":42}},"device_code":"test-device-code"}"#,
            ),
            (200, PROFILE),
            (200, PROFILE),
            (200, remote),
            (200, remote),
            (200, r#"{"result":{"status":"ok"}}"#),
            (200, merged),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut network = Network::new(&path);
        network.client = Client::with_base_url(url);
        let mut app = fixture();
        app.replace_favorites(BTreeSet::from([1, 2]));
        network.command(
            Command::Login {
                email: "test@example.com".into(),
                password: "test-pass".into(),
            },
            &mut app,
        );
        pump(&mut network, &mut app);
        assert_eq!(app.settings.favorites, BTreeSet::from([1, 2]));
        assert!(app.account.conflict.is_some());
        assert!(app.account.open);
        assert!(app.account.session.as_ref().unwrap().baseline.is_none());
        network.command(Command::Resolve(Choice::Merge), &mut app);
        pump(&mut network, &mut app);
        assert!(app.account.error.is_none());
        assert_eq!(app.settings.favorites, BTreeSet::from([1, 2, 3]));
        assert_eq!(
            app.account.session.as_ref().unwrap().baseline,
            Some(BTreeSet::from([1, 2, 3]))
        );
        let saved = Session::load(&network.path).unwrap().unwrap();
        assert_eq!(saved.user_id, 42);
        let stored = fs::read_to_string(&network.path).unwrap();
        assert!(!stored.contains("test-pass"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&network.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let requests = server.join().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.starts_with("POST /api/favorites/"))
                .count(),
            1
        );
    }

    #[test]
    fn local_and_server_choices_apply_exactly_the_reviewed_copy() {
        for choice in [Choice::Local, Choice::Server] {
            let remote = r#"{"result":{"stations":{"add":[2],"remove":[]}}}"#;
            let local = r#"{"result":{"stations":{"add":[1],"remove":[]}}}"#;
            let ok = r#"{"result":{"status":"ok"}}"#;
            let responses = if choice == Choice::Local {
                vec![(200, remote), (200, ok), (200, ok), (200, local)]
            } else {
                vec![(200, remote), (200, remote)]
            };
            let (url, server) = server(responses);
            let dir = tempfile::tempdir().unwrap();
            let mut network = Network::new(&dir.path().join("settings.json"));
            network.client = Client::with_base_url(url);
            let mut app = fixture();
            app.settings.favorites = BTreeSet::from([1]);
            app.account.session = Some(session(None));
            app.account.conflict = Some(Conflict {
                remote: BTreeSet::from([2]),
                choice,
            });
            network.command(Command::Resolve(choice), &mut app);
            pump(&mut network, &mut app);
            assert!(app.account.error.is_none());
            let expected = if choice == Choice::Local {
                BTreeSet::from([1])
            } else {
                BTreeSet::from([2])
            };
            assert_eq!(app.settings.favorites, expected);
            assert_eq!(
                app.account.session.as_ref().unwrap().baseline,
                Some(expected)
            );
            let requests = server.join().unwrap();
            if choice == Choice::Local {
                assert!(requests[1].starts_with("POST /api/favorites/station/?id=1 "));
                assert!(requests[2].starts_with("DELETE /api/favorites/station/?id=2 "));
            } else {
                assert!(requests.iter().all(|r| r.starts_with("GET ")));
            }
        }
    }

    #[test]
    fn automatic_sync_waits_for_edits_to_settle_and_pauses_after_errors() {
        let (url, server) = server(vec![
            (200, PROFILE),
            (200, r#"{"result":{"stations":{"add":[1,2],"remove":[]}}}"#),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let mut network = Network::new(&dir.path().join("settings.json"));
        network.client = Client::with_base_url(url);
        let mut app = fixture();
        app.account.session = Some(session(Some(&[1])));
        app.settings.favorites = BTreeSet::from([1, 2]);
        network.poll(&mut app);
        assert!(
            network.pending.is_none(),
            "do not sync while edits are still settling"
        );
        network.changed -= Duration::from_secs(1);
        app.account.error = Some("offline".into());
        network.poll(&mut app);
        assert!(network.pending.is_none(), "an error needs manual retry");
        app.account.error = None;
        network.poll(&mut app);
        assert!(app.account.busy);
        pump(&mut network, &mut app);
        assert_eq!(
            app.account.session.as_ref().unwrap().baseline,
            Some(BTreeSet::from([1, 2]))
        );
        server.join().unwrap();
    }

    #[test]
    fn restored_session_pulls_remote_only_changes_without_a_dialog() {
        let remote = r#"{"result":{"stations":{"add":[2,3],"remove":[]}}}"#;
        let (url, server) = server(vec![
            (200, PROFILE),
            (200, remote),
            (200, remote),
            (200, remote),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let account_path = dir.path().join("account.json");
        session(Some(&[1, 2])).save(&account_path).unwrap();
        let mut network = Network::new(&settings_path);
        network.client = Client::with_base_url(url);
        let mut app = fixture();
        app.settings.favorites = BTreeSet::from([1, 2]);
        network.restore(&mut app);
        pump(&mut network, &mut app);
        assert!(app.account.error.is_none());
        assert!(app.account.conflict.is_none());
        assert_eq!(app.settings.favorites, BTreeSet::from([2, 3]));
        assert!(server.join().unwrap().iter().all(|r| r.starts_with("GET ")));
    }

    #[test]
    fn remote_changes_after_review_are_replanned_before_any_writes() {
        let (url, server) = server(vec![
            (200, r#"{"result":{"stations":{"add":[2,5],"remove":[]}}}"#),
            (200, PROFILE),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let mut network = Network::new(&dir.path().join("settings.json"));
        network.client = Client::with_base_url(url);
        let mut app = fixture();
        app.settings.favorites = BTreeSet::from([1, 2]);
        app.account.session = Some(session(None));
        app.account.conflict = Some(Conflict {
            remote: BTreeSet::from([2, 3]),
            choice: Choice::Merge,
        });
        network.command(Command::Resolve(Choice::Local), &mut app);
        pump(&mut network, &mut app);
        assert_eq!(app.settings.favorites, BTreeSet::from([1, 2]));
        assert_eq!(
            app.account.conflict.as_ref().unwrap().remote,
            BTreeSet::from([2, 5])
        );
        assert!(
            server
                .join()
                .unwrap()
                .iter()
                .all(|request| request.starts_with("GET "))
        );
    }

    #[test]
    fn one_sided_changes_sync_without_prompt_and_keep_edits_made_in_flight() {
        let (url, server) = server(vec![
            (200, r#"{"result":{"stations":{"add":[1,2],"remove":[]}}}"#),
            (200, r#"{"result":{"status":"ok"}}"#),
            (
                200,
                r#"{"result":{"stations":{"add":[1,2,3],"remove":[]}}}"#,
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut network = Network::new(&path);
        network.client = Client::with_base_url(url);
        let mut app = fixture();
        app.settings.favorites = BTreeSet::from([1, 2, 3]);
        app.account.session = Some(session(Some(&[1, 2])));
        network.plan(&mut app, BTreeSet::from([1, 2]));
        assert!(app.account.conflict.is_none());
        assert!(app.account.busy);
        app.replace_favorites(BTreeSet::from([2, 3, 4]));
        pump(&mut network, &mut app);
        assert_eq!(app.settings.favorites, BTreeSet::from([2, 3, 4]));
        assert_eq!(
            app.account.session.as_ref().unwrap().baseline,
            Some(BTreeSet::from([1, 2, 3]))
        );
        assert_eq!(
            crate::settings::Settings::load(&path).unwrap().favorites,
            app.settings.favorites
        );
        server.join().unwrap();
    }

    #[test]
    fn failed_update_keeps_local_copy_and_last_successful_baseline() {
        let (url, server) = server(vec![
            (200, r#"{"result":{"stations":{"add":[2],"remove":[]}}}"#),
            (200, r#"{"result":{"status":"ok"}}"#),
            (503, r#"{"error":"failed"}"#),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let mut network = Network::new(&dir.path().join("settings.json"));
        network.client = Client::with_base_url(url);
        let mut app = fixture();
        app.settings.favorites = BTreeSet::from([1]);
        app.account.session = Some(session(Some(&[2])));
        network.plan(&mut app, BTreeSet::from([2]));
        pump(&mut network, &mut app);
        assert_eq!(app.settings.favorites, BTreeSet::from([1]));
        assert_eq!(
            app.account.session.as_ref().unwrap().baseline,
            Some(BTreeSet::from([2]))
        );
        assert!(app.account.error.as_ref().unwrap().contains("503"));
        assert!(app.account.last_sync.is_none());
        let requests = server.join().unwrap();
        assert!(requests[1].starts_with("POST "));
        assert!(requests[2].starts_with("DELETE "));
    }

    #[test]
    fn simultaneous_changes_prompt_and_deferral_does_not_write_anything() {
        let dir = tempfile::tempdir().unwrap();
        let mut network = Network::new(&dir.path().join("settings.json"));
        let mut app = fixture();
        app.settings.favorites = BTreeSet::from([1, 3]);
        app.account.session = Some(session(Some(&[1, 2])));
        network.plan(&mut app, BTreeSet::from([2, 4]));
        assert!(app.account.conflict.is_some());
        assert!(matches!(
            press(&mut app, KeyCode::Esc),
            crate::app::Action::None
        ));
        assert!(!app.account.open);
        network.poll(&mut app);
        assert!(network.pending.is_none());
        assert_eq!(app.settings.favorites, BTreeSet::from([1, 3]));
        assert!(!network.path.exists());
    }

    #[test]
    fn signing_out_cancels_pending_replies_and_preserves_local_favorites() {
        let dir = tempfile::tempdir().unwrap();
        let mut network = Network::new(&dir.path().join("settings.json"));
        let mut app = fixture();
        app.settings.favorites.insert(1);
        let previous = app.settings.favorites.clone();
        app.account.session = Some(session(Some(&[1])));
        network.save(&mut app);
        let (tx, rx) = mpsc::channel();
        network.pending = Some(rx);
        app.account.busy = true;
        let cancelled = network.cancelled.clone();
        network.command(Command::Logout, &mut app);
        assert!(cancelled.load(Ordering::Acquire));
        assert!(
            tx.send(Ok(Reply::Applied {
                session: session(Some(&[2])),
                before: previous.clone(),
                target: BTreeSet::from([2])
            }))
            .is_err()
        );
        network.poll(&mut app);
        assert!(app.account.session.is_none());
        assert_eq!(app.settings.favorites, previous);
        assert!(!network.path.exists());
    }

    #[test]
    fn login_input_is_isolated_and_password_is_cleared_on_submit_and_escape() {
        let mut app = fixture();
        press(&mut app, KeyCode::Char('a'));
        assert!(app.account.open);
        app.account.paste("test@example.com\r\n");
        press(&mut app, KeyCode::Tab);
        for ch in "qfs?".chars() {
            assert!(matches!(
                press(&mut app, KeyCode::Char(ch)),
                crate::app::Action::None
            ));
        }
        assert_eq!(app.account.password, "qfs?");
        assert!(app.settings.favorites.is_empty());
        assert!(!app.help);
        match press(&mut app, KeyCode::Enter) {
            crate::app::Action::Account(Command::Login { email, password }) => {
                assert_eq!(email, "test@example.com");
                assert_eq!(password, "qfs?");
            }
            _ => panic!("expected login"),
        }
        assert!(app.account.password.is_empty());
        app.account.paste("private");
        press(&mut app, KeyCode::Esc);
        assert!(app.account.password.is_empty());
        assert!(!app.account.open);
    }
}
