// Exercises cache admission and retained safety state.
use miniclaw_server::{chat::{Chat, Data, Receipt}, models::Choice};
// Builds synthetic summaries without upstream credentials.
use serde_json::json;

/// Fills the cache with active chats and one idle eviction candidate.
fn full_cache() -> Data {
    let mut data = Data::default();
    for index in 0..32 {
        let mut chat = Chat::new(json!({ "id": index.to_string() }));
        chat.history_loaded = true;
        if index != 0 { chat.active = Some("active-turn".into()); }
        data.chats.insert(index.to_string(), chat);
    }
    data
}

#[test]
/// Keeps active work, recovery state, and prior send receipts under cache pressure.
fn preserves_safety_state() {
    let guards: [fn(&mut Chat); 9] = [
        |chat| chat.history_loaded = false,
        |chat| chat.sending = true,
        |chat| chat.stopping = true,
        |chat| chat.uncertain = true,
        |chat| chat.computer_changing = true,
        |chat| chat.computer_error = Some("computer_reload"),
        |chat| chat.restore_pending = true,
        |chat| chat.summary["computerUse"] = json!(true),
        |chat| { chat.receipts.insert("client".into(), Receipt {
            text: "Question".into(), choice: Choice::default(), turn: None, attachments: Vec::new(),
        }); },
    ];
    for guard in guards {
        let mut data = full_cache();
        guard(data.chats.get_mut("0").unwrap());
        assert!(!data.make_room());
        assert_eq!(data.chats.len(), 32);
    }
}

#[test]
/// Releases a cache slot only after the last request or stream drops its lease.
fn reader_pins_cache() {
    let mut data = full_cache();
    let lease = data.chats.get_mut("0").unwrap().pin();
    assert!(!data.make_room());
    drop(lease);
    assert!(data.make_room());
    assert_eq!(data.chats.len(), 31);
    assert!(!data.chats.contains_key("0"));
}
