// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Child-session supervisor (Unix-only).
//!
//! When a headless `dapper proxy from-config` session has `childSessions` with
//! `autoSpawn`, this module spawns each child as its own peer `dapper proxy
//! from-config` process for the adapter's `startDebugging` reverse requests.
//! `SessionInitializer` sends a resolved child [`DebugSessionConfig`] over an
//! mpsc [`ChildSpawnRequest`]; the supervisor writes a hardened per-user temp
//! config, spawns the proxy, and only then acks (so the ack reflects a real spawn).
//!
//! Spawning is capped by `max_children` (fork-bomb safety) and tracked in a
//! registry; a per-child waiter reaps each child. [`ChildTeardown`] kills live
//! children before the parent exits; `PR_SET_PDEATHSIG` backstops a crashed parent.

use std::collections::HashMap;
use std::future::Future;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use async_trait::async_trait;
use dapper_control_server::ChildTeardownHook;
use dapper_proxy_server::ChildSpawnRequest;
use dapper_proxy_server::ProgressEvent;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::config::DebugSessionConfig;
use dapper_session::get_user_temp_dir;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tracing::debug;
use tracing::info;
use tracing::warn;
use uuid::Uuid;

/// Bound on the channel from `SessionInitializer`s to the supervisor. Spawning
/// is fast (`Command::spawn`), so this only smooths brief bursts.
const CHILD_SPAWN_CHANNEL_CAP: usize = 16;

/// Maximum attempts to create a uniquely-named temp config file before giving
/// up (each collision is astronomically unlikely with a UUID name).
const TEMP_FILE_NAME_ATTEMPTS: usize = 8;

/// Grace period between SIGTERM and the last-resort SIGKILL when tearing down a
/// child's process group.
const CHILD_TEARDOWN_GRACE: Duration = Duration::from_secs(2);

/// A child process spawned by a [`ChildSessionSpawner`]. The supervisor waits on
/// it (per-child waiter) and uses its metadata to track the live-child count.
///
/// This is a trait (rather than a concrete type) so the supervisor task can be
/// unit-tested with a fake child whose `wait` is driven by the test.
#[async_trait]
trait SpawnedChild: Send {
    /// PID of the spawned `dapper` proxy process.
    fn pid(&self) -> u32;
    /// Path of the temp config file to remove when the child exits.
    fn config_path(&self) -> &Path;
    /// Resolve when the child process exits.
    async fn wait(&mut self);
}

/// Abstraction over spawning a child `dapper proxy from-config` process, so the
/// supervisor task can be unit-tested with a fake spawner.
#[async_trait]
trait ChildSessionSpawner: Send + Sync {
    /// Spawn a child for `config`, returning a handle on success. The temp
    /// config file is written (and cleaned up on spawn failure) by the
    /// implementation.
    async fn spawn(&self, config: DebugSessionConfig) -> Result<Box<dyn SpawnedChild>>;
}

/// Metadata the supervisor retains for each live child. The actual process
/// handle is owned by that child's waiter task.
struct ChildEntry {
    pid: u32,
    config_path: PathBuf,
}

/// Registry of live children — the single source of truth for the concurrent
/// child count and per-child cleanup. Cheaply cloneable (shared via `Arc`).
#[derive(Clone, Default)]
struct ChildRegistry {
    inner: Arc<Mutex<ChildRegistryInner>>,
}

#[derive(Default)]
struct ChildRegistryInner {
    /// Keyed by child pid — unique among live children, since a pid can't be
    /// reused until its zombie is reaped and the waiter removes the entry at reap.
    children: HashMap<u32, ChildEntry>,
    /// Set once teardown begins. Guarded by the same lock as `children` so that
    /// "mark shutting down + drain" (teardown) and "check + insert" (supervisor)
    /// are atomic with respect to each other: a child whose spawn is in flight
    /// when teardown runs is therefore EITHER drained by teardown (it registered
    /// first) OR rejected at registration (teardown won), never orphaned.
    shutting_down: bool,
}

impl ChildRegistry {
    /// Lock the registry, recovering from a poisoned mutex rather than panicking
    /// (a panicked waiter must not wedge the supervisor).
    fn lock(&self) -> std::sync::MutexGuard<'_, ChildRegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Number of currently-tracked (live) children.
    fn live_count(&self) -> usize {
        self.lock().children.len()
    }

    /// Whether teardown has begun. A non-atomic peek used by the supervisor to
    /// reject *queued* requests early; the authoritative gate is
    /// [`insert_and_ack`], which checks the flag under the same lock as the insert.
    fn is_shutting_down(&self) -> bool {
        self.lock().shutting_down
    }

    /// Remove a child by pid, returning its entry if it was still tracked. The
    /// caller that gets `Some` is the single owner responsible for cleanup (so a
    /// waiter-vs-shutdown race can't double-clean or double-release a slot).
    fn remove(&self, pid: u32) -> Option<ChildEntry> {
        self.lock().children.remove(&pid)
    }

    /// Insert a just-spawned child (keyed by pid) and ack it to `reply`, all under
    /// one lock so the decision is atomic with `begin_shutdown`: once shutdown has
    /// begun we report failure, never a false success. Returns `true` if the child
    /// is now tracked; `false` means it was already replied-to (and rolled back),
    /// so the caller still owns the handle and must tear it down.
    fn insert_and_ack(&self, entry: ChildEntry, reply: oneshot::Sender<Result<()>>) -> bool {
        let mut inner = self.lock();
        if inner.shutting_down {
            warn!(
                pid = entry.pid,
                "shutdown raced child spawn; not tracking it"
            );
            let _ = reply.send(Err(anyhow!(
                "dapper is shutting down; child session aborted"
            )));
            return false;
        }
        let pid = entry.pid;
        inner.children.insert(pid, entry);
        if reply.send(Ok(())).is_err() {
            // Caller's bounded wait timed out; don't keep a session it believes failed.
            inner.children.remove(&pid);
            warn!(pid, "startDebugging caller gone before spawn confirmation");
            return false;
        }
        true
    }

    /// Track a child directly, bypassing the shutdown/ack handshake. Test-only —
    /// production inserts go through [`insert_and_ack`].
    #[cfg(test)]
    fn insert(&self, entry: ChildEntry) {
        self.lock().children.insert(entry.pid, entry);
    }

    /// Atomically mark the registry as shutting down and remove+return all
    /// tracked children. After this, [`insert_and_ack`] rejects every later
    /// child, so teardown cannot miss an in-flight spawn. Idempotent:
    /// a concurrent waiter sees an empty registry (single-owner cleanup) and a
    /// second call returns empty with the flag still set.
    fn begin_shutdown(&self) -> Vec<ChildEntry> {
        let mut inner = self.lock();
        inner.shutting_down = true;
        inner.children.drain().map(|(_, entry)| entry).collect()
    }
}

/// Tears down all live child sessions (children-before-parent shutdown). Cheaply
/// cloneable; shares the supervisor's registry.
#[derive(Clone)]
pub(crate) struct ChildTeardown {
    registry: ChildRegistry,
    /// Grace period between SIGTERM and the last-resort SIGKILL. A field (rather
    /// than reading the `CHILD_TEARDOWN_GRACE` const directly) so tests can
    /// shorten it and avoid a real multi-second wait.
    grace: Duration,
}

impl ChildTeardown {
    /// Tear down all live children: SIGTERM each child proxy's process group (so
    /// it disconnects its adapter), wait `grace`, then SIGKILL and remove each
    /// temp config. Idempotent (the registry drains atomically) and best-effort
    /// (`ESRCH` and cleanup errors are ignored so parent shutdown always
    /// proceeds).
    ///
    /// Limitation: `from-config` puts each child's adapter in its own session, so
    /// a wedged + SIGKILLed child proxy can orphan its adapter. A drained pid may
    /// also name a since-reaped child (pid reuse is slow enough to accept).
    pub(crate) async fn teardown(&self) {
        let children = self.registry.begin_shutdown();
        if children.is_empty() {
            return;
        }
        info!("tearing down {} child session(s)", children.len());

        for entry in &children {
            signal_child_group(entry.pid, libc::SIGTERM);
        }
        tokio::time::sleep(self.grace).await;
        for entry in &children {
            signal_child_group(entry.pid, libc::SIGKILL);
            if let Err(e) = tokio::fs::remove_file(&entry.config_path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(
                    "failed to remove child temp config {}: {e}",
                    entry.config_path.display()
                );
            }
        }
    }
}

/// Build a control-plane teardown hook from a [`ChildTeardown`], for wiring into
/// `start_control_plane` and the shutdown cascade.
pub(crate) fn teardown_hook(teardown: ChildTeardown) -> ChildTeardownHook {
    Arc::new(move || {
        let teardown = teardown.clone();
        Box::pin(async move { teardown.teardown().await })
            as Pin<Box<dyn Future<Output = ()> + Send>>
    })
}

/// Send `signal` to a child proxy's process group (the child is a `setsid`
/// session/group leader, so its pgid equals its pid). `ESRCH` (the group is
/// already gone) is treated as success.
fn signal_child_group(pid: u32, signal: libc::c_int) {
    // Negate the pid to target the process group. Linux pids fit in i32; if one
    // somehow doesn't, skip rather than wrap into a bogus target.
    let Ok(pid) = i32::try_from(pid) else {
        warn!("child pid {pid} exceeds i32::MAX; skipping group signal");
        return;
    };
    // SAFETY: `kill` with a negative pid targets a process group and has no
    // memory-safety preconditions; we inspect errno for anything but ESRCH.
    let rc = unsafe { libc::kill(-pid, signal) };
    if rc == -1 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            warn!("failed to signal child process group {pid} (signal {signal}): {err}");
        }
    }
}

/// Tear down a child the supervisor spawned but will not track — either the
/// caller's bounded wait timed out, or teardown began while the spawn was in
/// flight. SIGKILLs the child's process group, removes its temp config, and
/// reaps it (detached so the supervisor loop keeps serving).
async fn teardown_spawned_child(child: Box<dyn SpawnedChild>) {
    let config_path = child.config_path().to_path_buf();
    // SIGKILL the child's process group (it is its own `setsid` leader, so
    // pgid == pid); `signal_child_group` tolerates `ESRCH` and logs other errors.
    signal_child_group(child.pid(), libc::SIGKILL);
    if let Err(e) = tokio::fs::remove_file(&config_path).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!(
            "failed to remove orphaned child temp config {}: {e}",
            config_path.display()
        );
    }
    // Reap detached so the supervisor loop keeps serving; the SIGKILL above makes
    // this `wait` return promptly for a real child.
    tokio::spawn(async move {
        let mut child = child;
        child.wait().await;
    });
}

/// Run the supervisor task: serve [`ChildSpawnRequest`]s until the channel
/// closes. Each request is enforced against `max_children`, spawned via
/// `spawner`, and acked on its oneshot once the process has spawned.
async fn run_child_supervisor(
    mut rx: mpsc::Receiver<ChildSpawnRequest>,
    spawner: Arc<dyn ChildSessionSpawner>,
    max_children: u32,
    registry: ChildRegistry,
) {
    while let Some(ChildSpawnRequest { config, reply }) = rx.recv().await {
        // If the caller already gave up (its bounded wait timed out and dropped
        // the receiver), drop the request without side effects — never spawn a
        // child that nobody is waiting on.
        if reply.is_closed() {
            continue;
        }

        // Reject queued requests once teardown has begun — don't start new spawns
        // while the parent is shutting down. (The authoritative gate is the
        // post-spawn `insert_and_ack`, which also closes the window where shutdown
        // begins *during* the spawn below.)
        if registry.is_shutting_down() {
            let _ = reply.send(Err(anyhow!(
                "dapper is shutting down; not spawning child session"
            )));
            continue;
        }

        // Enforce the concurrent-child cap. The supervisor processes requests
        // sequentially, so this check-then-insert can only race with waiters
        // removing entries — which frees slots, never consumes them.
        let live = registry.live_count();
        if live >= max_children as usize {
            let _ = reply.send(Err(anyhow!(
                "max concurrent child sessions reached ({live}/{max_children})"
            )));
            continue;
        }

        match spawner.spawn(config).await {
            Ok(child) => {
                // Insert + ack atomically against teardown (one registry lock,
                // shared with `begin_shutdown`): the child is either tracked or
                // rejected (already replied), never orphaned. If not tracked we
                // still own the handle, so tear it down.
                let entry = ChildEntry {
                    pid: child.pid(),
                    config_path: child.config_path().to_path_buf(),
                };
                if registry.insert_and_ack(entry, reply) {
                    spawn_child_waiter(registry.clone(), child);
                } else {
                    teardown_spawned_child(child).await;
                }
            }
            Err(e) => {
                let _ = reply.send(Err(e));
            }
        }
    }
    debug!("child-spawn channel closed; supervisor task exiting");
}

/// Spawn a task that waits for `child` to exit, then removes it from the
/// registry (releasing its `max_children` slot) and deletes its temp config.
fn spawn_child_waiter(registry: ChildRegistry, mut child: Box<dyn SpawnedChild>) {
    tokio::spawn(async move {
        let pid = child.pid();
        child.wait().await;
        // Single-owner remove-then-act: only whoever removes the entry cleans up.
        if let Some(entry) = registry.remove(pid) {
            debug!(
                pid = entry.pid,
                "child session exited; cleaning up temp config"
            );
            if let Err(e) = tokio::fs::remove_file(&entry.config_path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(
                    "failed to remove child temp config {}: {e}",
                    entry.config_path.display()
                );
            }
        }
    });
}

/// Wire up the child-session supervisor for a headless proxy, returning the
/// channel a `SessionInitializer` uses to request child spawns. Returns `None`
/// (child spawning disabled) when `childSessions` is absent, `autoSpawn` is off,
/// the depth/child budget is zero, or the dapper binary path can't be
/// determined. In all those cases the reverse-request handler fails closed.
pub(crate) fn setup_child_supervisor(
    config: &DebugSessionConfig,
    parent_session_id: &SessionId,
    scope_id: Option<ScopeId>,
) -> Option<(mpsc::Sender<ChildSpawnRequest>, ChildTeardown)> {
    let child_sessions = config.child_sessions.as_ref()?;
    if !child_sessions.auto_spawn
        || child_sessions.max_children == 0
        || child_sessions.max_depth == 0
    {
        return None;
    }

    let dapper_bin = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            warn!("cannot determine dapper binary path; child sessions disabled: {e}");
            return None;
        }
    };

    let (tx, rx) = mpsc::channel(CHILD_SPAWN_CHANNEL_CAP);
    let spawner: Arc<dyn ChildSessionSpawner> = Arc::new(OsChildSpawner {
        dapper_bin,
        scope_id,
        parent_session_id: parent_session_id.clone(),
    });
    let registry = ChildRegistry::default();
    let teardown = ChildTeardown {
        registry: registry.clone(),
        grace: CHILD_TEARDOWN_GRACE,
    };
    tokio::spawn(run_child_supervisor(
        rx,
        spawner,
        child_sessions.max_children,
        registry,
    ));
    Some((tx, teardown))
}

/// Spawns child sessions as real peer `dapper proxy from-config` OS processes.
struct OsChildSpawner {
    /// Path to the `dapper` binary (this process's executable).
    dapper_bin: PathBuf,
    /// Scope id to give children (the parent proxy's scope).
    scope_id: Option<ScopeId>,
    /// The parent proxy's session id, passed to children as `--parent-session-id`.
    parent_session_id: SessionId,
}

#[async_trait]
impl ChildSessionSpawner for OsChildSpawner {
    async fn spawn(&self, config: DebugSessionConfig) -> Result<Box<dyn SpawnedChild>> {
        let dir = ensure_child_config_dir().await?;
        let config_path = write_child_config(&dir, &config).await?;

        match self.spawn_proxy(&config_path).await {
            Ok(child) => Ok(child),
            Err(e) => {
                // Don't leak the temp config if the spawn itself failed.
                let _ = tokio::fs::remove_file(&config_path).await;
                Err(e)
            }
        }
    }
}

impl OsChildSpawner {
    async fn spawn_proxy(&self, config_path: &Path) -> Result<Box<dyn SpawnedChild>> {
        // Events pipe: std marks both ends CLOEXEC (no leak into the child);
        // `pre_exec` clears it on the inherited write end.
        let (events_reader, events_writer) = std::io::pipe().context("creating events pipe")?;
        let write_fd = events_writer.as_raw_fd();

        let mut cmd = Command::new(&self.dapper_bin);
        cmd.arg("proxy");
        if let Some(scope) = &self.scope_id {
            cmd.arg("--scope-id").arg(scope.as_str());
        }
        cmd.arg("--parent-session-id")
            .arg(self.parent_session_id.as_str());
        // Dynamic control-plane port so children never collide with the parent.
        cmd.arg("--control-port").arg("0");
        cmd.arg("from-config");
        // `--events-fd` and the config path are `from-config` subcommand args.
        cmd.arg("--events-fd").arg(write_fd.to_string());
        cmd.arg(config_path);
        // The child must never inherit the parent's stdio: the parent uses its
        // own stdout for `[DAPPER_SESSION]` event lines, and structured child
        // events flow over `--events-fd` instead.
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // The supervisor process's pid, captured before fork, so the child can
        // detect a parent death that races the `prctl` below.
        let parent_pid = std::process::id() as i32;

        // SAFETY: `pre_exec` runs post-`fork()`, pre-`exec()`, so it may call
        // only async-signal-safe functions. `fcntl`/`setsid`/`getppid`/`raise`
        // are on the POSIX list; `signal` and `prctl` aren't formally listed but
        // are single syscalls with no libc state. Captured vars are plain `i32`s.
        unsafe {
            cmd.pre_exec(move || {
                // Clear FD_CLOEXEC on the events write end so it survives `exec`
                // for the child to write events. The child re-sets it on consume,
                // so the fd isn't leaked into its own grandchildren.
                let flags = libc::fcntl(write_fd, libc::F_GETFD);
                if flags == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(write_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // Put the child in its own session/process group so teardown can
                // target the group, and so it can't steal the terminal's
                // foreground process group.
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // Reset SIGTERM to default before arming PDEATHSIG: the child
                // inherited the parent runtime's SIGTERM handler, which is unsafe
                // to run post-fork, so any SIGTERM (from PDEATHSIG or the `raise`
                // below) must just terminate.
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                // Defense-in-depth: have the kernel SIGTERM this child if the
                // parent thread exits (explicit teardown is the primary path).
                // Linux-only — macOS lacks `prctl(PR_SET_PDEATHSIG)`.
                #[cfg(target_os = "linux")]
                if libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGTERM as libc::c_ulong,
                    0,
                    0,
                    0,
                ) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                // Close the fork/prctl race: if the parent already exited (we've
                // been reparented), PDEATHSIG won't fire, so self-terminate.
                if libc::getppid() != parent_pid {
                    libc::raise(libc::SIGTERM);
                }
                Ok(())
            });
        }

        // On spawn failure both pipe ends drop here, closing their fds.
        let mut child = cmd.spawn().context("failed to spawn child dapper proxy")?;

        // Drop the parent's copy of the write end so the read end observes EOF
        // once the child (the only remaining writer) exits.
        drop(events_writer);

        let pid = child
            .id()
            .expect("a freshly spawned child has a pid before it is waited on");

        // Drain the child's stdout/stderr into tracing so a full pipe can't
        // block the child.
        if let Some(stdout) = child.stdout.take() {
            spawn_log_drain(stdout, "stdout", pid);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_drain(stderr, "stderr", pid);
        }
        // Drain the structured events pipe: parse each line and log it. The
        // events are informational here — teardown signals the child's process
        // group rather than relying on this stream.
        spawn_events_drain(events_reader, pid);

        info!(pid, "spawned child dapper proxy");
        Ok(Box::new(OsSpawnedChild {
            pid,
            config_path: config_path.to_path_buf(),
            child,
        }))
    }
}

/// An OS-process-backed [`SpawnedChild`].
struct OsSpawnedChild {
    pid: u32,
    config_path: PathBuf,
    child: Child,
}

#[async_trait]
impl SpawnedChild for OsSpawnedChild {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn config_path(&self) -> &Path {
        &self.config_path
    }

    async fn wait(&mut self) {
        if let Err(e) = self.child.wait().await {
            warn!(pid = self.pid, "error waiting for child dapper proxy: {e}");
        }
    }
}

/// Create-or-validate the per-user child-config directory, returning its path.
///
/// The directory holds debugger args/metadata, so this is a best-effort attempt
/// to keep it private: we validate symlink-aware (lstat, never following links),
/// require current-user ownership and mode `0700`, and fail closed otherwise.
///
/// This raises the bar but is not airtight: the `get_user_temp_dir()` base is not
/// validated, so a local attacker who controls it could swap this subdir in the
/// window between validation and the later write (a TOCTOU race). That residual,
/// hard-to-win local race is out of scope here.
async fn ensure_child_config_dir() -> Result<PathBuf> {
    let base = get_user_temp_dir();
    // Best-effort create the per-user base; the security checks below are on the
    // child-config subdir we actually write into.
    let _ = tokio::fs::create_dir_all(&base).await;

    let dir = base.join("child_session_configs");
    match tokio::fs::create_dir(&dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            return Err(e).with_context(|| format!("creating child config dir {}", dir.display()));
        }
    }

    validate_child_config_dir(&dir).await?;
    Ok(dir)
}

/// Validate `dir` is a real, current-user-owned, mode-`0700` directory
/// (repairing the mode, failing closed otherwise). Symlink-aware (`lstat`), so a
/// swapped-in symlink is detected, not traversed.
async fn validate_child_config_dir(dir: &Path) -> Result<()> {
    let meta = tokio::fs::symlink_metadata(dir)
        .await
        .with_context(|| format!("stat child config dir {}", dir.display()))?;
    if meta.file_type().is_symlink() {
        bail!(
            "child config dir {} is a symlink; refusing to use it",
            dir.display()
        );
    }
    if !meta.is_dir() {
        bail!("child config path {} is not a directory", dir.display());
    }
    // SAFETY: `getuid` has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if meta.uid() != uid {
        bail!(
            "child config dir {} is not owned by the current user",
            dir.display()
        );
    }
    if meta.permissions().mode() & 0o777 != 0o700 {
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .await
            .with_context(|| format!("setting 0700 on child config dir {}", dir.display()))?;
        let meta = tokio::fs::symlink_metadata(dir).await?;
        if meta.permissions().mode() & 0o777 != 0o700 {
            bail!(
                "failed to enforce 0700 on child config dir {}",
                dir.display()
            );
        }
    }
    Ok(())
}

/// Write `config` to a uniquely-named, mode-`0600`, exclusively-created file in
/// `dir`, returning its path. Uses `create_new` (`O_EXCL`) so it never follows
/// or truncates an existing file/symlink, and `mode(0o600)` at create time so
/// there is no world-readable window.
async fn write_child_config(dir: &Path, config: &DebugSessionConfig) -> Result<PathBuf> {
    let json = serde_json::to_vec(config).context("serializing child session config")?;

    for _ in 0..TEMP_FILE_NAME_ATTEMPTS {
        let path = dir.join(format!("child-{}.json", Uuid::new_v4()));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                if let Err(e) = async {
                    file.write_all(&json).await?;
                    file.flush().await
                }
                .await
                {
                    // Don't leak a partially-written file: the caller only cleans
                    // up on spawn failure and never receives this path.
                    let _ = tokio::fs::remove_file(&path).await;
                    return Err(e)
                        .with_context(|| format!("writing child config {}", path.display()));
                }
                return Ok(path);
            }
            // Astronomically unlikely UUID collision: try a fresh name.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("creating child config file {}", path.display()));
            }
        }
    }
    bail!(
        "could not create a unique child config file in {}",
        dir.display()
    )
}

/// Drain a child stdout/stderr stream into tracing, line by line, until EOF.
fn spawn_log_drain<R>(reader: R, stream: &'static str, pid: u32)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            debug!(pid, stream, "child: {line}");
        }
    });
}

/// Drain the structured events pipe: parse each line as a [`ProgressEvent`] and
/// log it. Runs on a blocking thread because it reads the pipe to EOF. Owns the
/// read end (closed when it drops at EOF).
fn spawn_events_drain(reader: std::io::PipeReader, pid: u32) {
    tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<ProgressEvent>(&line) {
                Ok(event) => debug!(pid, ?event, "child progress event"),
                Err(_) => debug!(pid, "child events line: {line}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use tokio::sync::broadcast;
    use tokio::sync::oneshot;

    use super::*;

    /// A fake child whose `wait` resolves when the shared release channel fires.
    struct FakeChild {
        pid: u32,
        config_path: PathBuf,
        release: broadcast::Receiver<()>,
    }

    #[async_trait]
    impl SpawnedChild for FakeChild {
        fn pid(&self) -> u32 {
            self.pid
        }
        fn config_path(&self) -> &Path {
            &self.config_path
        }
        async fn wait(&mut self) {
            // Resolve on the first signal (or if the sender is dropped).
            let _ = self.release.recv().await;
        }
    }

    /// A fake spawner that records how many times it was called and hands each
    /// child a subscription to a shared release channel.
    struct FakeSpawner {
        spawn_count: Arc<AtomicUsize>,
        release: broadcast::Sender<()>,
    }

    #[async_trait]
    impl ChildSessionSpawner for FakeSpawner {
        async fn spawn(&self, _config: DebugSessionConfig) -> Result<Box<dyn SpawnedChild>> {
            let n = self.spawn_count.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeChild {
                // Non-existent path: the waiter's cleanup tolerates NotFound.
                pid: 1000 + n as u32,
                config_path: PathBuf::from(format!("/nonexistent/fake-child-{n}.json")),
                release: self.release.subscribe(),
            }))
        }
    }

    fn dummy_config() -> DebugSessionConfig {
        serde_json::from_str(r#"{ "spawnConfig": { "type": "stdio", "cmd": "x" } }"#).unwrap()
    }

    async fn request_spawn(tx: &mpsc::Sender<ChildSpawnRequest>) -> Result<()> {
        let (reply, reply_rx) = oneshot::channel();
        tx.send(ChildSpawnRequest {
            config: dummy_config(),
            reply,
        })
        .await
        .expect("supervisor receiver alive");
        reply_rx.await.expect("supervisor replied")
    }

    #[tokio::test]
    async fn test_max_children_cap_and_slot_release() {
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let (release, _keep) = broadcast::channel(4);
        let spawner: Arc<dyn ChildSessionSpawner> = Arc::new(FakeSpawner {
            spawn_count: spawn_count.clone(),
            release: release.clone(),
        });
        let registry = ChildRegistry::default();
        let (tx, rx) = mpsc::channel(8);
        let max_children = 2;
        let supervisor = tokio::spawn(run_child_supervisor(
            rx,
            spawner,
            max_children,
            registry.clone(),
        ));

        // Spawns up to the cap succeed.
        assert!(request_spawn(&tx).await.is_ok());
        assert!(request_spawn(&tx).await.is_ok());
        assert_eq!(registry.live_count(), 2, "both children tracked");

        // The next request exceeds the cap: rejected without invoking the spawner.
        assert!(
            request_spawn(&tx).await.is_err(),
            "third spawn must be rejected by the max_children cap"
        );
        assert_eq!(
            spawn_count.load(Ordering::SeqCst),
            2,
            "spawner must not be called for a capped-out request"
        );

        // Releasing the live children frees their slots (waiters remove them).
        let _ = release.send(());
        let mut released = false;
        for _ in 0..100 {
            if registry.live_count() == 0 {
                released = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(released, "slots should be released after children exit");

        // A new spawn now fits under the cap again.
        assert!(request_spawn(&tx).await.is_ok());
        assert_eq!(registry.live_count(), 1);

        drop(tx);
        let _ = supervisor.await;
    }

    #[tokio::test]
    async fn validate_child_config_dir_rejects_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = validate_child_config_dir(&link)
            .await
            .expect_err("a symlinked config dir must be rejected");
        assert!(
            err.to_string().contains("symlink"),
            "error should mention symlink, got: {err}"
        );
    }

    #[tokio::test]
    async fn validate_child_config_dir_rejects_non_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();

        let err = validate_child_config_dir(&file)
            .await
            .expect_err("a non-directory config path must be rejected");
        assert!(
            err.to_string().contains("not a directory"),
            "error should mention non-directory, got: {err}"
        );
    }

    #[tokio::test]
    async fn validate_child_config_dir_repairs_loose_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("loose");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();

        validate_child_config_dir(&dir)
            .await
            .expect("a current-user-owned dir with loose mode should be repaired, not rejected");

        let mode = std::fs::symlink_metadata(&dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "loose mode must be tightened to 0700");
    }

    #[tokio::test]
    async fn write_child_config_creates_private_unique_files() {
        let tmp = tempfile::tempdir().unwrap();
        let config: DebugSessionConfig =
            serde_json::from_str(r#"{ "spawnConfig": { "type": "stdio", "cmd": "echo" } }"#)
                .unwrap();

        let p1 = write_child_config(tmp.path(), &config).await.unwrap();
        let p2 = write_child_config(tmp.path(), &config).await.unwrap();
        assert_ne!(p1, p2, "each call must create a distinct file");

        for p in [&p1, &p2] {
            let mode = std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "child config must be created mode 0600");
            // The written file deserializes back to an equivalent config.
            let roundtrip: DebugSessionConfig =
                serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(&roundtrip).unwrap(),
                serde_json::to_value(&config).unwrap()
            );
        }
    }

    /// A spawner that signals when its `spawn` is entered, then blocks on `gate`
    /// before returning a child. The child uses a pid at the top of the pid space
    /// that is overwhelmingly unlikely to name a live process group (so a teardown
    /// SIGKILL is effectively a no-op — and the test keys on temp-config removal,
    /// not on the kill) and a caller-given temp config path (so removal is
    /// observable).
    struct GatedSpawner {
        entered: Arc<tokio::sync::Notify>,
        gate: Arc<tokio::sync::Notify>,
        pid: u32,
        config_path: PathBuf,
        release: broadcast::Sender<()>,
    }

    #[async_trait]
    impl ChildSessionSpawner for GatedSpawner {
        async fn spawn(&self, _config: DebugSessionConfig) -> Result<Box<dyn SpawnedChild>> {
            self.entered.notify_one();
            self.gate.notified().await;
            Ok(Box::new(FakeChild {
                pid: self.pid,
                config_path: self.config_path.clone(),
                release: self.release.subscribe(),
            }))
        }
    }

    #[tokio::test]
    async fn test_caller_gone_before_spawn_is_skipped() {
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let (release, _rx) = broadcast::channel(4);
        let spawner: Arc<dyn ChildSessionSpawner> = Arc::new(FakeSpawner {
            spawn_count: spawn_count.clone(),
            release,
        });
        let registry = ChildRegistry::default();
        let (tx, rx) = mpsc::channel(8);
        let supervisor = tokio::spawn(run_child_supervisor(rx, spawner, 4, registry.clone()));

        // A request whose caller has already given up: drop the receiver before
        // the supervisor processes it.
        let (reply, reply_rx) = oneshot::channel();
        drop(reply_rx);
        tx.send(ChildSpawnRequest {
            config: dummy_config(),
            reply,
        })
        .await
        .expect("supervisor receiver alive");

        // A following normal request: once its reply arrives, the prior request
        // (FIFO channel, sequential supervisor) has already been processed.
        assert!(request_spawn(&tx).await.is_ok());

        assert_eq!(
            spawn_count.load(Ordering::SeqCst),
            1,
            "the abandoned request must be skipped before spawning"
        );
        assert_eq!(registry.live_count(), 1, "only the live request is tracked");

        drop(tx);
        let _ = supervisor.await;
    }

    #[tokio::test]
    async fn test_spawn_confirmed_caller_gone_tears_down_child() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("orphan.json");
        std::fs::write(&config_path, b"{}").unwrap();

        let entered = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(tokio::sync::Notify::new());
        let (release, _rx) = broadcast::channel(1);
        let spawner: Arc<dyn ChildSessionSpawner> = Arc::new(GatedSpawner {
            entered: entered.clone(),
            gate: gate.clone(),
            // Top of the pid space — overwhelmingly unlikely to name a live
            // process group, so the teardown SIGKILL is effectively a no-op.
            pid: 0x7FFF_FFFE,
            config_path: config_path.clone(),
            release,
        });
        let registry = ChildRegistry::default();
        let (tx, rx) = mpsc::channel(8);
        let supervisor = tokio::spawn(run_child_supervisor(rx, spawner, 4, registry.clone()));

        let (reply, reply_rx) = oneshot::channel();
        tx.send(ChildSpawnRequest {
            config: dummy_config(),
            reply,
        })
        .await
        .expect("supervisor receiver alive");

        // Wait until the spawn is in flight (past the is_closed pre-check), then
        // simulate a caller timeout by dropping the receiver before the spawn
        // completes.
        entered.notified().await;
        drop(reply_rx);
        gate.notify_one();

        // The caller is gone (its receiver was dropped), so once the spawn
        // completes the supervisor registers the child, its ack `send` fails, and
        // it then unregisters and tears the child down — leaving live_count back
        // at 0 with the temp config removed. We poll because that unregister +
        // removal happen after the gate is released.
        let mut torn_down = false;
        for _ in 0..200 {
            if registry.live_count() == 0 && !config_path.exists() {
                torn_down = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            torn_down,
            "orphaned child must be torn down: live={}, config_exists={}",
            registry.live_count(),
            config_path.exists()
        );

        drop(tx);
        let _ = supervisor.await;
    }

    #[test]
    fn test_begin_shutdown_is_idempotent_and_blocks_inserts() {
        let registry = ChildRegistry::default();
        registry.insert(ChildEntry {
            pid: 4242,
            config_path: PathBuf::from("/nonexistent/a.json"),
        });
        registry.insert(ChildEntry {
            pid: 4243,
            config_path: PathBuf::from("/nonexistent/b.json"),
        });
        assert_eq!(registry.live_count(), 2);
        assert!(!registry.is_shutting_down());

        // First call drains all, empties the registry, and latches the flag.
        let drained = registry.begin_shutdown();
        assert_eq!(
            drained.len(),
            2,
            "begin_shutdown returns all tracked children"
        );
        assert_eq!(registry.live_count(), 0);
        assert!(registry.is_shutting_down());

        // Second call is a no-op, and a stale remove finds nothing (single-owner).
        assert!(
            registry.begin_shutdown().is_empty(),
            "second begin_shutdown is a no-op"
        );
        assert!(
            registry.remove(4242).is_none(),
            "remove after begin_shutdown finds nothing (single-owner cleanup)"
        );

        // Once shutting down, insert_and_ack rejects the child and replies failure.
        let (reply, mut reply_rx) = oneshot::channel();
        assert!(
            !registry.insert_and_ack(
                ChildEntry {
                    pid: 4244,
                    config_path: PathBuf::from("/nonexistent/c.json"),
                },
                reply,
            ),
            "inserts must be rejected once shutdown has begun"
        );
        assert!(reply_rx.try_recv().expect("replied").is_err());
        assert_eq!(registry.live_count(), 0);
    }

    #[tokio::test]
    async fn test_insert_and_ack_tracked_shutdown_and_caller_gone() {
        // Tracked: a live receiver gets Ok and the child stays tracked.
        let registry = ChildRegistry::default();
        let (reply, reply_rx) = oneshot::channel();
        assert!(registry.insert_and_ack(
            ChildEntry {
                pid: 4242,
                config_path: PathBuf::from("/nonexistent/a.json"),
            },
            reply,
        ));
        assert!(reply_rx.await.expect("acked").is_ok());
        assert_eq!(registry.live_count(), 1);

        // Shutting down: insert_and_ack rejects and reports failure rather than a
        // false success — the atomicity guarantee (no `Ok` once shutdown began).
        let registry = ChildRegistry::default();
        registry.begin_shutdown();
        let (reply, reply_rx) = oneshot::channel();
        assert!(!registry.insert_and_ack(
            ChildEntry {
                pid: 4243,
                config_path: PathBuf::from("/nonexistent/b.json"),
            },
            reply,
        ));
        assert!(reply_rx.await.expect("replied").is_err());
        assert_eq!(registry.live_count(), 0);

        // Caller gone: the receiver was dropped (caller timed out), so the entry
        // is rolled back.
        let registry = ChildRegistry::default();
        let (reply, reply_rx) = oneshot::channel();
        drop(reply_rx);
        assert!(!registry.insert_and_ack(
            ChildEntry {
                pid: 4244,
                config_path: PathBuf::from("/nonexistent/c.json"),
            },
            reply,
        ));
        assert_eq!(registry.live_count(), 0, "caller-gone child is rolled back");
    }

    #[tokio::test]
    async fn test_teardown_empty_is_noop() {
        // No children: teardown returns promptly without signaling anything.
        let teardown = ChildTeardown {
            registry: ChildRegistry::default(),
            grace: Duration::from_millis(0),
        };
        teardown.teardown().await;
        assert_eq!(teardown.registry.live_count(), 0);
    }

    #[tokio::test]
    async fn test_teardown_removes_configs_and_is_single_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ChildRegistry::default();

        // Distinct pids near the top of the pid space, so neither names a live
        // group (the SIGKILL no-ops on ESRCH); this tests the drain + cleanup.
        let mut paths = Vec::new();
        for (i, name) in ["a.json", "b.json"].into_iter().enumerate() {
            let path = tmp.path().join(name);
            std::fs::write(&path, b"{}").unwrap();
            registry.insert(ChildEntry {
                pid: 0x7FFF_FFFE - i as u32,
                config_path: path.clone(),
            });
            paths.push(path);
        }
        assert_eq!(registry.live_count(), 2);

        let teardown = ChildTeardown {
            registry: registry.clone(),
            grace: Duration::from_millis(0),
        };

        // Two concurrent teardown calls: only one drains the registry (single
        // owner), so the temp configs are removed exactly once and the registry
        // ends empty regardless of which call won the drain.
        let (t1, t2) = (teardown.clone(), teardown.clone());
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move { t1.teardown().await }),
            tokio::spawn(async move { t2.teardown().await }),
        );
        r1.unwrap();
        r2.unwrap();

        assert_eq!(registry.live_count(), 0, "registry must be fully drained");
        for path in &paths {
            assert!(
                !path.exists(),
                "temp config {} should be removed by teardown",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn test_queued_request_after_shutdown_is_not_spawned() {
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let (release, _rx) = broadcast::channel(4);
        let spawner: Arc<dyn ChildSessionSpawner> = Arc::new(FakeSpawner {
            spawn_count: spawn_count.clone(),
            release,
        });
        let registry = ChildRegistry::default();
        // Shutdown has already begun before the request is processed.
        assert!(registry.begin_shutdown().is_empty());
        let (tx, rx) = mpsc::channel(8);
        let supervisor = tokio::spawn(run_child_supervisor(rx, spawner, 4, registry.clone()));

        // A request enqueued after shutdown began must be rejected without ever
        // calling the spawner.
        let (reply, reply_rx) = oneshot::channel();
        tx.send(ChildSpawnRequest {
            config: dummy_config(),
            reply,
        })
        .await
        .expect("supervisor receiver alive");
        let result = reply_rx.await.expect("supervisor replied");

        assert!(result.is_err(), "a request after shutdown must be rejected");
        assert_eq!(
            spawn_count.load(Ordering::SeqCst),
            0,
            "the spawner must not be called once shutdown has begun"
        );
        assert_eq!(registry.live_count(), 0);

        drop(tx);
        let _ = supervisor.await;
    }

    #[tokio::test]
    async fn test_in_flight_spawn_during_shutdown_is_torn_down() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("inflight.json");
        std::fs::write(&config_path, b"{}").unwrap();

        let entered = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(tokio::sync::Notify::new());
        let (release, _rx) = broadcast::channel(1);
        let spawner: Arc<dyn ChildSessionSpawner> = Arc::new(GatedSpawner {
            entered: entered.clone(),
            gate: gate.clone(),
            pid: 0x7FFF_FFFE,
            config_path: config_path.clone(),
            release,
        });
        let registry = ChildRegistry::default();
        let (tx, rx) = mpsc::channel(8);
        let supervisor = tokio::spawn(run_child_supervisor(rx, spawner, 4, registry.clone()));

        let (reply, reply_rx) = oneshot::channel();
        tx.send(ChildSpawnRequest {
            config: dummy_config(),
            reply,
        })
        .await
        .expect("supervisor receiver alive");

        // Once the spawn is in flight (past the pre-spawn shutdown check), begin
        // shutdown while it's blocked, then let the spawn complete.
        entered.notified().await;
        assert!(
            registry.begin_shutdown().is_empty(),
            "the in-flight child is not registered yet, so the drain is empty"
        );
        gate.notify_one();

        // The child produced after shutdown must be torn down — never acked or
        // tracked. (`live_count` stays 0 since it is never registered; the config
        // removal is the proof teardown actually ran.)
        let result = reply_rx.await.expect("supervisor replied");
        assert!(
            result.is_err(),
            "a child produced during shutdown must not be acked"
        );
        let mut torn_down = false;
        for _ in 0..200 {
            if registry.live_count() == 0 && !config_path.exists() {
                torn_down = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            torn_down,
            "in-flight child must be torn down: live={}, config_exists={}",
            registry.live_count(),
            config_path.exists()
        );

        drop(tx);
        let _ = supervisor.await;
    }

    /// Exercises the *real* process-group signaling path (not the sentinel-pid
    /// no-op the other teardown tests use): spawn a genuine long-lived child in
    /// its own session/process group (via `setsid`), so `signal_child_group`'s
    /// `kill(-pid, …)` targets only that child's group — never the test runner —
    /// then assert `teardown` actually signals it dead and removes its temp
    /// config.
    #[tokio::test]
    async fn test_teardown_signals_and_reaps_real_child_process() {
        use std::os::unix::process::CommandExt;
        use std::os::unix::process::ExitStatusExt;

        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("real-child.json");
        std::fs::write(&config_path, b"{}").unwrap();

        // A real child that blocks until signaled, placed in its own
        // session/process group so the group-targeted kill cannot reach the
        // test's own process group.
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("120");
        // SAFETY: `setsid` is async-signal-safe and is the only call made in the
        // forked child before `exec`.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("spawn real child process (sleep)");
        let pid = child.id();

        let registry = ChildRegistry::default();
        registry.insert(ChildEntry {
            pid,
            config_path: config_path.clone(),
        });
        let teardown = ChildTeardown {
            registry: registry.clone(),
            grace: Duration::from_millis(100),
        };

        teardown.teardown().await;

        // The child must have been killed by teardown's signal; reap it and
        // confirm it was terminated by a signal (SIGTERM within the grace
        // window, or the last-resort SIGKILL).
        let status = child.wait().expect("reap signaled child");
        assert!(
            status.signal().is_some(),
            "teardown must terminate the real child via a signal, got: {status:?}"
        );
        assert_eq!(registry.live_count(), 0, "registry must be drained");
        assert!(
            !config_path.exists(),
            "teardown must remove the child's temp config"
        );
    }
}
