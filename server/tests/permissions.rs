// Checks persisted grants through the private store.
use miniclaw_server::agents::Store;
// Names exact operations and their authorizing sources.
use miniclaw_server::permissions::{Context, Decision, Effect, Scope, Target};
// Seeds only durable bindings needed by authorization.
use rusqlite::{Connection, params};
// Creates isolated permission stores for each test.
use std::{fs, path::PathBuf};
// Generates unrelated identities for every test.
use uuid::Uuid;

struct Fixture(PathBuf);

impl Fixture {
    /// Opens a private, disposable store and returns its cleanup owner.
    fn open() -> (Self, Store) {
        let path = std::env::temp_dir().join(format!("mc-permissions-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        let store = Store::open(&path.join("agents.sqlite3")).unwrap();
        (Self(path), store)
    }

    /// Saves an admitted conversation so authorization derives every identity.
    async fn bind(&self, store: &Store, agent: Option<Uuid>, project: Option<Uuid>,
        machine: &str) -> Context {
        let thread = Uuid::new_v4().to_string();
        let db = Connection::open(self.0.join("agents.sqlite3")).unwrap();
        if let Some(agent) = agent {
            db.execute("INSERT OR IGNORE INTO agents VALUES (?1, '{}')", [agent.to_string()])
                .unwrap();
            db.execute("INSERT INTO agent_chats VALUES (?1, ?2, '{}')",
                params![thread, agent.to_string()]).unwrap();
        }
        if let Some(project) = project {
            db.execute("INSERT OR IGNORE INTO projects VALUES (?1, '{}')", [project.to_string()])
                .unwrap();
            db.execute("INSERT INTO project_chats VALUES (?1, ?2, '[]')",
                params![thread, project.to_string()]).unwrap();
        }
        let assignment = if machine == "host" {
            "{\"kind\":\"host\"}".to_owned()
        } else {
            format!("{{\"kind\":\"vm\",\"vmId\":\"{machine}\"}}")
        };
        db.execute("INSERT INTO chat_environments VALUES (?1, ?2)",
            params![thread, assignment]).unwrap();
        let required = 8 + i64::from(agent.is_some()) + 2 * i64::from(project.is_some());
        db.execute("INSERT INTO chat_admissions VALUES (?1, ?2)",
            params![thread, required]).unwrap();
        store.permission_context(thread).await.unwrap()
    }
}

impl Drop for Fixture {
    /// Removes only this fixture's UUID-named temporary directory.
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Names one command without normalizing its reviewed text or directory.
fn command() -> Target {
    Target::Command {
        kind: "shell".into(),
        text: "printf approved".into(),
        cwd: "/tmp/fixture".into(),
    }
}

#[tokio::test]
async fn conversation_block_overrides_inherited_grants() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), Some(Uuid::new_v4()),
        "master").await;
    let target = command();
    assert_eq!(store.permission(&context, &target, 100).await.unwrap(), Decision::Ask);

    store.set_permission(Scope::Agent(context.agent().unwrap()), &target, Some(Effect::Allow))
        .await.unwrap();
    assert_eq!(store.permission(&context, &target, 100).await.unwrap(),
        Decision::Allow(Scope::Agent(context.agent().unwrap())));

    store.set_permission(Scope::Conversation(context.conversation()), &target,
        Some(Effect::Block)).await.unwrap();
    assert_eq!(store.permission(&context, &target, 100).await.unwrap(), Decision::Block);

    store.set_permission(Scope::Conversation(context.conversation()), &target, None)
        .await.unwrap();
    assert_eq!(store.permission(&context, &target, 100).await.unwrap(),
        Decision::Allow(Scope::Agent(context.agent().unwrap())));
}

#[tokio::test]
async fn session_grant_is_exact_and_expires_after_eight_hours() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), Some(Uuid::new_v4()),
        "master").await;
    let target = command();
    store.approve_session(&context, &target, 1_000).await.unwrap();
    assert_eq!(store.permission(&context, &target, 1_000).await.unwrap(),
        Decision::Allow(Scope::Session));
    drop(store);
    let store = Store::open(&fixture.0.join("agents.sqlite3")).unwrap();
    assert_eq!(store.permission(&context, &target, 29_799).await.unwrap(),
        Decision::Allow(Scope::Session));
    assert_eq!(store.permission(&context, &target, 29_800).await.unwrap(), Decision::Ask);

    let mut changed = fixture.bind(&store, context.agent(), context.project(),
        "master").await;
    assert_eq!(store.permission(&changed, &target, 1_001).await.unwrap(), Decision::Ask);
    let db = Connection::open(fixture.0.join("agents.sqlite3")).unwrap();
    db.execute("UPDATE chat_environments SET assignment = '{\"kind\":\"host\"}'
        WHERE thread_id = ?1", [context.conversation().to_string()]).unwrap();
    changed = store.permission_context(context.conversation().to_string()).await.unwrap();
    assert_eq!(store.permission(&changed, &target, 1_001).await.unwrap(), Decision::Ask);
    let changed_target = Target::Command {
        kind: "shell".into(), text: "printf denied".into(), cwd: "/tmp/fixture".into(),
    };
    assert_eq!(store.permission(&context, &changed_target, 1_001).await.unwrap(), Decision::Ask);
}

#[tokio::test]
async fn revocation_clears_session_and_future_access() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), Some(Uuid::new_v4()),
        "master").await;
    let target = command();
    store.approve_session(&context, &target, 1_000).await.unwrap();
    store.revoke_permission(Scope::Conversation(context.conversation()), &target)
        .await.unwrap();
    assert_eq!(store.permission(&context, &target, 1_001).await.unwrap(), Decision::Ask);
}

#[tokio::test]
async fn context_comes_from_saved_chat_bindings() {
    let (fixture, store) = Fixture::open();
    let expected = fixture.bind(&store, Some(Uuid::new_v4()), Some(Uuid::new_v4()),
        "master").await;
    assert_eq!(store.permission_context(expected.conversation().to_string()).await.unwrap(),
        expected);
    assert!(store.permission_context(Uuid::new_v4().to_string()).await.is_err());
}

#[tokio::test]
async fn deleting_chat_removes_its_permissions() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), None, "master").await;
    let target = command();
    store.set_permission(Scope::Conversation(context.conversation()), &target,
        Some(Effect::Allow)).await.unwrap();
    store.approve_session(&context, &target, 1_000).await.unwrap();
    store.request_application(Scope::Conversation(context.conversation()), "Preview")
        .await.unwrap();

    store.remove_chat(context.conversation().to_string()).await.unwrap();
    let db = Connection::open(fixture.0.join("agents.sqlite3")).unwrap();
    let rules: i64 = db.query_row("SELECT count(*) FROM permission_rules
        WHERE scope_kind = 'conversation' AND scope_id = ?1",
        [context.conversation().to_string()], |row| row.get(0)).unwrap();
    let sessions: i64 = db.query_row("SELECT count(*) FROM permission_sessions
        WHERE conversation = ?1", [context.conversation().to_string()],
        |row| row.get(0)).unwrap();
    let requests: i64 = db.query_row("SELECT count(*) FROM permission_app_requests
        WHERE scope_kind = 'conversation' AND scope_id = ?1",
        [context.conversation().to_string()], |row| row.get(0)).unwrap();
    assert_eq!((rules, sessions, requests), (0, 0, 0));
}

#[tokio::test]
async fn settings_show_session_only_grants_with_expiry() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, None, None, "master").await;
    let target = command();
    store.approve_session(&context, &target, 1_000).await.unwrap();
    let summary = store.permission_summary(&context, 1_001).await.unwrap();
    assert_eq!(summary.sources.session.len(), 1);
    assert_eq!(summary.sources.session[0].target, target);
    assert_eq!(summary.sources.session[0].expires_at, 29_800);
    assert_eq!(summary.effective.len(), 1);
    assert_eq!(summary.effective[0].decision, "allow");
    assert_eq!(summary.effective[0].source, "session");
    assert!(store.permission_summary(&context, 29_800).await.unwrap()
        .sources.session.is_empty());
}

#[tokio::test]
async fn unvetted_app_cannot_gain_access_from_rules_or_sessions() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), None, "master").await;
    let unknown = Target::Application {
        id: "com.example.Unreviewed".into(), action: "control".into(),
    };
    assert_eq!(store.permission(&context, &unknown, 1_000).await.unwrap(),
        Decision::Unavailable);
    assert!(store.set_permission(Scope::Agent(context.agent().unwrap()), &unknown,
        Some(Effect::Allow)).await.is_err());
    assert!(store.approve_session(&context, &unknown, 1_000).await.is_err());

    let db = Connection::open(fixture.0.join("agents.sqlite3")).unwrap();
    db.execute("INSERT INTO permission_rules VALUES ('agent', ?1, ?2, 'allow')",
        params![context.agent().unwrap().to_string(),
            serde_json::to_string(&unknown).unwrap()]).unwrap();
    assert_eq!(store.permission(&context, &unknown, 1_001).await.unwrap(),
        Decision::Unavailable);
}

/// A restart must not bring back access the user took away.
#[tokio::test]
async fn a_revocation_survives_a_restart() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), Some(Uuid::new_v4()),
        "master").await;
    let target = command();
    let scope = Scope::Conversation(context.conversation());
    store.set_permission(scope, &target, Some(Effect::Allow)).await.unwrap();
    store.approve_session(&context, &target, 1_000).await.unwrap();
    store.revoke_permission(scope, &target).await.unwrap();
    drop(store);
    let store = Store::open(&fixture.0.join("agents.sqlite3")).unwrap();
    assert_eq!(store.permission(&context, &target, 1_001).await.unwrap(), Decision::Ask);
    // A block saved before the restart still wins over an agent's allow afterwards.
    store.set_permission(Scope::Agent(context.agent().unwrap()), &target, Some(Effect::Allow))
        .await.unwrap();
    store.set_permission(scope, &target, Some(Effect::Block)).await.unwrap();
    drop(store);
    let store = Store::open(&fixture.0.join("agents.sqlite3")).unwrap();
    assert_eq!(store.permission(&context, &target, 1_002).await.unwrap(), Decision::Block);
}

#[tokio::test]
async fn app_request_survives_restart_without_becoming_a_grant() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, None, None, "master").await;
    let scope = Scope::Conversation(context.conversation());
    store.request_application(scope, "Preview").await.unwrap();
    assert_eq!(store.application_requests(scope).await.unwrap().len(), 1);
    drop(store);
    let store = Store::open(&fixture.0.join("agents.sqlite3")).unwrap();
    let requests = store.application_requests(scope).await.unwrap();
    assert_eq!(requests[0].name, "Preview");
    assert_eq!(requests[0].status, "pending");
    let target = Target::Application {
        id: "com.apple.Preview".into(), action: "control".into(),
    };
    assert_eq!(store.permission(&context, &target, 1_000).await.unwrap(),
        Decision::Unavailable);
}

#[tokio::test]
async fn app_request_queue_is_bounded_and_deduplicated() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, None, None, "master").await;
    let scope = Scope::Conversation(context.conversation());
    store.request_application(scope, "Preview").await.unwrap();
    store.request_application(scope, "preview").await.unwrap();
    assert_eq!(store.application_requests(scope).await.unwrap().len(), 1);
    for index in 1..256 {
        store.request_application(scope, &format!("App {index}")).await.unwrap();
    }
    assert!(store.request_application(scope, "App 256").await.is_err());
    assert_eq!(store.application_requests(scope).await.unwrap().len(), 256);
}

#[tokio::test]
async fn rules_reject_lexical_path_escape() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, None, None, "master").await;
    let scope = Scope::Conversation(context.conversation());
    for target in [
        Target::File { path: "/tmp/../private".into(), action: "read".into() },
        Target::Command { kind: "shell".into(), text: "true".into(),
            cwd: "/tmp/../private".into() },
    ] {
        assert!(store.set_permission(scope, &target, Some(Effect::Allow)).await.is_err());
    }
}

#[test]
fn missing_permission_table_stops_reopen() {
    let (fixture, store) = Fixture::open();
    drop(store);
    let db = Connection::open(fixture.0.join("agents.sqlite3")).unwrap();
    db.execute("DROP TABLE permission_sessions", []).unwrap();
    drop(db);
    assert!(Store::open(&fixture.0.join("agents.sqlite3")).is_err());
}

#[test]
fn incomplete_v4_policy_cannot_migrate_as_a_fresh_catalog() {
    let (fixture, store) = Fixture::open();
    drop(store);
    let db = Connection::open(fixture.0.join("agents.sqlite3")).unwrap();
    db.execute_batch("DROP TABLE permission_rules;
        DROP TABLE permission_app_requests; PRAGMA user_version = 4;").unwrap();
    drop(db);
    assert!(Store::open(&fixture.0.join("agents.sqlite3")).is_err());
}

/// A host is approved by its exact name; a domain rule covers the domain and what is below it.
#[tokio::test]
async fn a_host_rule_is_exact_and_a_domain_rule_covers_its_subdomains() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), None, "host").await;
    let scope = Scope::Conversation(context.conversation());
    let host = |name: &str| Target::Host { host: name.into() };
    store.set_permission(scope, &host("registry.npmjs.org"), Some(Effect::Allow)).await.unwrap();
    assert!(matches!(store.permission(&context, &host("registry.npmjs.org"), 1).await.unwrap(),
        Decision::Allow(_)));
    assert_eq!(store.permission(&context, &host("www.npmjs.org"), 1).await.unwrap(), Decision::Ask);
    assert_eq!(store.permission(&context, &host("evilregistry.npmjs.org.example.com"), 1)
        .await.unwrap(), Decision::Ask);
    store.set_permission(scope, &Target::Domain { domain: "githubusercontent.com".into() },
        Some(Effect::Allow)).await.unwrap();
    for covered in ["githubusercontent.com", "objects.githubusercontent.com",
        "a.b.githubusercontent.com"] {
        assert!(matches!(store.permission(&context, &host(covered), 1).await.unwrap(),
            Decision::Allow(_)), "{covered}");
    }
    // A name that merely ends with the same letters is another domain.
    assert_eq!(store.permission(&context, &host("notgithubusercontent.com"), 1).await.unwrap(),
        Decision::Ask);
    // An exact block inside an allowed domain still wins.
    store.set_permission(scope, &host("raw.githubusercontent.com"), Some(Effect::Block)).await.unwrap();
    assert_eq!(store.permission(&context, &host("raw.githubusercontent.com"), 1).await.unwrap(),
        Decision::Block);
}

/// Only plain lowercase DNS names are hosts: no address, port, path, wildcard or bare label.
#[test]
fn host_and_domain_names_are_checked() {
    for good in ["registry.npmjs.org", "pypi.org", "a-b.example.co.uk"] {
        assert!(Target::Host { host: good.into() }.validate().is_ok(), "{good}");
    }
    for bad in ["localhost", "127.0.0.1", "[::1]", "10.0.0.5", "example.com:8443",
        "example.com/path", "*.example.com", "EXAMPLE.com", "exa mple.com", "-a.example.com",
        "a..example.com", "", "com"] {
        assert!(Target::Host { host: bad.into() }.validate().is_err(), "{bad}");
    }
    // A domain rule must be narrower than a public suffix or a shared hosting domain.
    for bad in ["com", "co.uk", "github.io", "amazonaws.com", "*.example.com"] {
        assert!(Target::Domain { domain: bad.into() }.validate().is_err(), "{bad}");
    }
    assert!(Target::Domain { domain: "npmjs.org".into() }.validate().is_ok());
    assert_eq!(miniclaw_server::permissions::parent_domain("registry.npmjs.org").as_deref(),
        Some("npmjs.org"));
    assert_eq!(miniclaw_server::permissions::parent_domain("example.co.uk"), None);
    assert_eq!(miniclaw_server::permissions::parent_domain("user.github.io"), None);
    assert_eq!(miniclaw_server::permissions::parent_domain("npmjs.org"), None);
}

/// A switch is off without a rule, available with an allow from the project or the agent, and
/// on only once the conversation itself allows it; its block is stored as no rule.
#[tokio::test]
async fn a_feature_switch_is_off_by_default_and_the_conversation_has_the_last_word() {
    let (fixture, store) = Fixture::open();
    let context = fixture.bind(&store, Some(Uuid::new_v4()), Some(Uuid::new_v4()), "host").await;
    let network = Target::Feature { name: "network".into() };
    assert_eq!(store.permission(&context, &network, 1).await.unwrap(), Decision::Ask);
    let conversation = Scope::Conversation(context.conversation());
    assert!(store.set_permission(conversation, &network, Some(Effect::Allow)).await.is_err(), "nothing allows it yet");
    store.set_permission(Scope::Project(context.project().unwrap()), &network, Some(Effect::Allow))
        .await.unwrap();
    assert_eq!(store.permission(&context, &network, 1).await.unwrap(), Decision::Ask, "allowed is not on");
    store.set_permission(conversation, &network, Some(Effect::Allow)).await.unwrap();
    assert_eq!(store.permission(&context, &network, 1).await.unwrap(), Decision::Allow(conversation));
    store.set_permission(conversation, &network, Some(Effect::Block)).await.unwrap();
    assert_eq!(store.permission(&context, &network, 1).await.unwrap(), Decision::Ask);
    assert!(Target::Feature { name: "root_access".into() }.validate().is_err());
}

/// A project folder is accepted only as the path it resolves to; a link to it, a trailing
/// slash, a file and a missing folder are refused.
#[cfg(unix)]
#[test]
fn project_folder_must_be_its_resolved_path() {
    use miniclaw_server::permissions::exact_folder;
    let root = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("mc-project-folder-{}", Uuid::new_v4()));
    fs::create_dir_all(root.join("Project")).unwrap();
    fs::write(root.join("Project/file.txt"), "x").unwrap();
    std::os::unix::fs::symlink(root.join("Project"), root.join("Alias")).unwrap();
    let folder = root.join("Project").to_str().unwrap().to_owned();
    assert!(exact_folder(&folder));
    for alias in [root.join("Alias").to_str().unwrap().to_owned(), format!("{folder}/"), format!("{folder}/file.txt"),
        format!("{folder}/missing"), format!("{folder}/../Project")] {
        assert!(!exact_folder(&alias), "{alias}");
    }
    // On macOS the temporary folder is usually reached through the `/var` link.
    let linked = std::env::temp_dir().join(root.file_name().unwrap()).join("Project");
    if linked.to_str() != Some(folder.as_str()) { assert!(!exact_folder(linked.to_str().unwrap()), "{linked:?}"); }
    fs::remove_dir_all(root).unwrap();
}
