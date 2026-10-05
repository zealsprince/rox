//! Plugin actions (ADR 30's actions amendment): calling `source.action`, and
//! polling the jobs it starts until they end.
//!
//! A job is the plugin's work; rox only asks how far it got. Every call is
//! one rox makes, so a plugin still never sends anything unasked.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, Task};
use rox_plugins::wire::{self, ActionAnswer, JobState};
use serde_json::{Value, json};

use crate::plugins::{await_first_apply, host_for, running};

pub use rox_plugins::manifest::ActionDecl;
pub use rox_plugins::wire::Outcome;

const POLL: Duration = Duration::from_secs(1);

/// How long a plugin has after Stop to report the job ended, before rox
/// stops asking.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// Each source's row flags as its listings and actions last gave them, and
/// how many action reports there have been. A panel showing the source's rows
/// merges them when the count moves, so a menu reflects what an action just
/// did without listing the place again. A library row's menu reads them too,
/// since the row itself carries no flags.
static FLAGGED: LazyLock<Mutex<HashMap<String, Flagged>>> = LazyLock::new(Default::default);

/// Moves whenever any source's flags do, listed or reported, so a holder of
/// one row's flags knows to read them again.
static FLAGS_MOVED: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Flagged {
    reports: u64,
    flags: HashMap<String, Vec<String>>,
}

/// The source's reported flags, when there have been reports since `seen`.
pub fn flags_since(source: &str, seen: u64) -> Option<(u64, HashMap<String, Vec<String>>)> {
    let flagged = FLAGGED.lock().ok()?;
    let found = flagged.get(source)?;

    (found.reports != seen).then(|| (found.reports, found.flags.clone()))
}

/// The newest flags this session saw for the source's rows, listed or reported.
pub fn known_flags(source: &str) -> HashMap<String, Vec<String>> {
    flags_since(source, u64::MAX)
        .map(|(_, flags)| flags)
        .unwrap_or_default()
}

/// One row's newest flags, None when nobody said.
pub fn flags_for(source: &str, item: &str) -> Option<Vec<String>> {
    FLAGGED.lock().ok()?.get(source)?.flags.get(item).cloned()
}

pub fn flags_moved() -> u64 {
    FLAGS_MOVED.load(Ordering::Relaxed)
}

pub(crate) fn bump_flags() {
    FLAGS_MOVED.fetch_add(1, Ordering::Relaxed);
}

/// A listing's flags. Not a report: the panel that listed them already has
/// them, so nothing else needs to merge.
pub fn note(source: &str, flags: &HashMap<String, Vec<String>>) {
    if flags.is_empty() {
        return;
    }

    if let Ok(mut flagged) = FLAGGED.lock() {
        let found = flagged.entry(source.to_string()).or_default();
        found
            .flags
            .extend(flags.iter().map(|(item, now)| (item.clone(), now.clone())));
    }
    bump_flags();
}

pub(crate) fn report(source: &str, flags: HashMap<String, Vec<String>>) {
    if flags.is_empty() {
        return;
    }

    if let Ok(mut flagged) = FLAGGED.lock() {
        let found = flagged.entry(source.to_string()).or_default();
        found.reports += 1;
        found.flags.extend(flags);
    }
    bump_flags();
}

/// The actions a running plugin declares. Empty when it isn't running.
pub fn actions(source: &str) -> Vec<ActionDecl> {
    running(source)
        .and_then(|running| {
            let manifest = running.host.manifest();
            manifest
                .capabilities
                .source
                .as_ref()
                .map(|cap| cap.actions.clone())
        })
        .unwrap_or_default()
}

pub enum Started {
    Done(Outcome),
    Job(Arc<Job>),
}

/// Asks the plugin to run `action` on `items`, its tracks' keys or nodes'
/// ids, empty for an action on no item.
pub fn run(
    source: &str,
    action: &ActionDecl,
    items: Vec<String>,
    params: Value,
    cx: &App,
) -> Task<Result<Started, String>> {
    let source = source.to_string();
    let action = action.clone();

    cx.background_executor()
        .spawn(async move { start(source, action, items, params) })
}

fn start(
    source: String,
    action: ActionDecl,
    items: Vec<String>,
    params: Value,
) -> Result<Started, String> {
    await_first_apply(&source);
    let host = host_for(&source)?;

    let params = json!({ "action": action.id, "items": items, "params": params });
    let answer = host.call("source.action", params, host.timeouts().listing)?;
    if answer.is_null() {
        return Ok(Started::Done(Outcome::default()));
    }

    let mut answer: ActionAnswer = wire::decode(answer)?;
    report(&source, std::mem::take(&mut answer.flags));

    let Some(id) = answer.job.clone() else {
        return Ok(Started::Done(answer.outcome()));
    };

    let plugin = running(&source)
        .map(|running| running.label.clone())
        .unwrap_or_else(|| source.clone());

    let job = Arc::new(Job {
        serial: SERIAL.fetch_add(1, Ordering::Relaxed),
        source,
        plugin,
        label: action.label,
        id,
        done: AtomicU64::new(0),
        total: AtomicU64::new(0),
        text: Mutex::new(answer.message),
        stop: AtomicBool::new(false),
    });
    JOBS.lock().unwrap().push(job.clone());

    Ok(Started::Job(job))
}

/// A job a plugin is running, for the Tasks window's row.
pub struct Job {
    /// Unique for the session, for keying the job's row.
    pub serial: u64,
    pub source: String,
    /// The plugin's source label.
    pub plugin: String,
    /// The action's label.
    pub label: String,
    id: String,
    done: AtomicU64,
    total: AtomicU64,
    text: Mutex<String>,
    stop: AtomicBool,
}

impl Job {
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// Zero when the plugin can't tell.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }

    /// Asks the plugin to stop it, on the next poll.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

static JOBS: LazyLock<Mutex<Vec<Arc<Job>>>> = LazyLock::new(|| Mutex::new(Vec::new()));

static SERIAL: AtomicU64 = AtomicU64::new(1);

/// Every job still running, oldest first.
pub fn jobs() -> Vec<Arc<Job>> {
    JOBS.lock().unwrap().clone()
}

pub fn job(serial: u64) -> Option<Arc<Job>> {
    jobs().into_iter().find(|job| job.serial == serial)
}

pub enum JobEnd {
    Finished(Outcome),
    Failed(String),
    Stopped,
}

/// Polls `job` until the plugin reports it ended, the plugin goes away, or a
/// Stop runs out of grace.
pub fn watch(job: Arc<Job>, cx: &App) -> Task<JobEnd> {
    let executor = cx.background_executor().clone();

    cx.background_executor().spawn(async move {
        let mut stop_sent: Option<Instant> = None;

        let end = loop {
            executor.timer(POLL).await;

            if job.stopping() && stop_sent.is_none() {
                stop_sent = Some(Instant::now());

                // The plugin reports the end through the next polls; a failed
                // cancel only means it ends the long way.
                if let Err(e) = call(&job, "source.cancel") {
                    log::warn!("{}: cancel job {}: {e}", job.source, job.id);
                }
            }

            let mut state = match call(&job, "source.job").and_then(wire::decode::<JobState>) {
                Ok(state) => state,
                Err(_) if stop_sent.is_some() => break JobEnd::Stopped,
                Err(e) => break JobEnd::Failed(e),
            };

            // A plugin sends them as the job ends, stopped or not.
            report(&job.source, std::mem::take(&mut state.flags));

            job.done.store(state.done, Ordering::Relaxed);
            job.total.store(state.total, Ordering::Relaxed);
            *job.text.lock().unwrap() = state.text.clone();

            match (state.error.clone(), state.finished) {
                (Some(_), _) if stop_sent.is_some() => break JobEnd::Stopped,
                (Some(error), _) => break JobEnd::Failed(error),
                (None, true) => break JobEnd::Finished(state.outcome()),
                (None, false) => {}
            }

            if stop_sent.is_some_and(|sent| sent.elapsed() > STOP_GRACE) {
                break JobEnd::Stopped;
            }
        };

        JOBS.lock()
            .unwrap()
            .retain(|running| !Arc::ptr_eq(running, &job));

        end
    })
}

fn call(job: &Job, method: &'static str) -> Result<Value, String> {
    let host = host_for(&job.source)?;
    host.call(method, json!({ "job": job.id }), host.timeouts().listing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(item: &str, now: &[&str]) -> HashMap<String, Vec<String>> {
        HashMap::from([(
            item.to_string(),
            now.iter().map(|f| f.to_string()).collect(),
        )])
    }

    #[test]
    fn a_library_row_reads_the_newest_flags_listed_or_reported() {
        let source = "plugin:flags-test";
        assert!(known_flags(source).is_empty());

        note(source, &flags("t1", &["online"]));
        assert_eq!(known_flags(source)["t1"], ["online"]);
        assert_eq!(
            flags_since(source, 0),
            None,
            "a listing isn't news to the panel that listed it"
        );

        report(source, flags("t1", &["favourite", "online"]));
        assert_eq!(known_flags(source)["t1"], ["favourite", "online"]);
        assert_eq!(flags_since(source, 0).map(|(seen, _)| seen), Some(1));
    }
}
