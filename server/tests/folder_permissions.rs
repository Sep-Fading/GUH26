// Checks that a folder grant covers the files inside it.
use miniclaw_server::{agents::Store, environment::Assignment,
    permissions::{Decision, Effect, Scope, Target}};
// Names the disposable store.
use uuid::Uuid;

/// Builds a file or folder target for one action.
fn target(folder: bool, path: &str, action: &str) -> Target {
    if folder { Target::Folder { path: path.into(), action: action.into() } }
    else { Target::File { path: path.into(), action: action.into() } }
}

#[tokio::test]
async fn folder_rules_cover_contained_files_only() {
    let dir = std::env::temp_dir().join(format!("mc-folder-{}", Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let store = Store::open(&dir.join("agents.sqlite3")).unwrap();
    let thread = Uuid::new_v4().to_string();
    store.assign_existing(thread.clone(), Assignment::Host {}).await.unwrap();
    let context = store.permission_context(thread.clone()).await.unwrap();
    let scope = Scope::Conversation(context.conversation());
    store.set_permission(scope, &target(true, "/work/project", "read"), Some(Effect::Allow))
        .await.unwrap();
    let decide = async |path: &str, action: &str| store.permission(&context,
        &target(false, path, action), 1).await.unwrap();
    assert!(matches!(decide("/work/project/src/main.rs", "read").await, Decision::Allow(_)));
    assert_eq!(decide("/work/project/src/main.rs", "write").await, Decision::Ask);
    assert_eq!(decide("/work/project-other/a.txt", "read").await, Decision::Ask);
    assert_eq!(decide("/work/a.txt", "read").await, Decision::Ask);
    // An exact block inside an allowed folder still wins.
    store.set_permission(scope, &target(false, "/work/project/.env", "read"), Some(Effect::Block))
        .await.unwrap();
    assert_eq!(decide("/work/project/.env", "read").await, Decision::Block);
    let grants = store.file_grants(&context, 1).await.unwrap();
    let allowed: Vec<_> = grants.iter().filter(|grant| !grant.blocked).collect();
    assert_eq!(allowed.len(), 1);
    assert!(allowed[0].folder && !allowed[0].write && allowed[0].path == "/work/project");
    assert!(grants.iter().any(|grant| grant.blocked && grant.path == "/work/project/.env"));
    std::fs::remove_dir_all(dir).unwrap();
}
