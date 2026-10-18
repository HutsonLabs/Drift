//! Which agent serves a connection, and which agents the daemon accepts (task **HOST-13**,
//! ADR `HOST-13-login-window`). Pure.
//!
//! The daemon only brokers: it accepts TCP connections and hands each one to an agent. The
//! agent for the user at the console runs in that user's GUI session (`Aqua`); when nobody is
//! logged in, the login-window agent (`LoginWindow`, run as root by launchd) takes it.

use std::collections::BTreeMap;

/// The launchd session an agent runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SessionKind {
    /// A logged-in user's GUI session.
    Aqua,
    /// The login window, before anyone logs in.
    LoginWindow,
}

/// Who is at the console (`SCDynamicStoreCopyConsoleUser`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleState {
    /// The login window is showing.
    LoginWindow,
    /// A user is at the console.
    User {
        /// Their uid.
        uid: u32,
        /// Their short name.
        name: String,
    },
    /// The console could not be read.
    Unknown,
}

/// An agent registered with the daemon.
pub type AgentId = u64;

/// Why a connection is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefuseReason {
    /// The console could not be read: refuse rather than guess whose session to show.
    NoConsole,
}

/// What to do with a new connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Hand it to this agent.
    Agent(AgentId),
    /// The right agent is not there yet (it is starting); hold the connection for a while.
    Wait,
    /// Close it.
    Refuse(RefuseReason),
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    kind: SessionKind,
    uid: u32,
}

/// The agents connected to the daemon. Ids increase, so the newest agent for a session wins
/// (a restarted agent replaces one whose socket has not closed yet).
#[derive(Debug, Default, Clone)]
pub struct AgentTable {
    next: AgentId,
    agents: BTreeMap<AgentId, Entry>,
}

impl AgentTable {
    /// Adds an admitted agent and returns its id.
    pub fn register(&mut self, kind: SessionKind, uid: u32) -> AgentId {
        self.next += 1;
        self.agents.insert(self.next, Entry { kind, uid });
        self.next
    }

    /// Removes an agent (its socket closed).
    pub fn remove(&mut self, id: AgentId) {
        self.agents.remove(&id);
    }

    /// How many agents are connected.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Whether no agent is connected.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    fn newest(&self, want: impl Fn(&Entry) -> bool) -> Option<AgentId> {
        self.agents.iter().rev().find(|(_, e)| want(e)).map(|(id, _)| *id)
    }
}

/// The agent for `console`, if it is connected.
pub fn route(console: &ConsoleState, agents: &AgentTable) -> Route {
    let found = match console {
        ConsoleState::Unknown => return Route::Refuse(RefuseReason::NoConsole),
        ConsoleState::LoginWindow => agents.newest(|e| e.kind == SessionKind::LoginWindow),
        ConsoleState::User { uid, .. } => agents.newest(|e| e.kind == SessionKind::Aqua && e.uid == *uid),
    };
    found.map_or(Route::Wait, Route::Agent)
}

/// Why the daemon turned an agent away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentRefused {
    /// The socket's peer is not the user the agent claims to serve.
    #[error("the agent's uid does not match the user it claims")]
    UidMismatch,
    /// A user-session agent must not run as root.
    #[error("a user-session agent must not run as root")]
    RootAqua,
    /// The login-window agent must be the one launchd starts as root.
    #[error("the login-window agent must run as root")]
    NotRoot,
}

/// Which peers the daemon accepts as agents (checked against `getpeereid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentPolicy {
    root_uid: u32,
}

impl AgentPolicy {
    /// The real policy: the login-window agent is root (uid 0).
    pub const fn system() -> Self {
        Self { root_uid: 0 }
    }

    /// A test policy where `uid` stands in for root (tests run unprivileged).
    pub const fn with_root_uid(uid: u32) -> Self {
        Self { root_uid: uid }
    }

    /// Admits an agent of `kind` whose socket peer is `peer_uid` and who claims `claimed_uid`.
    pub fn admit(&self, kind: SessionKind, peer_uid: u32, claimed_uid: u32) -> Result<(), AgentRefused> {
        match kind {
            SessionKind::LoginWindow if peer_uid == self.root_uid => Ok(()),
            SessionKind::LoginWindow => Err(AgentRefused::NotRoot),
            SessionKind::Aqua if peer_uid != claimed_uid => Err(AgentRefused::UidMismatch),
            SessionKind::Aqua if peer_uid == 0 => Err(AgentRefused::RootAqua),
            SessionKind::Aqua => Ok(()),
        }
    }
}
