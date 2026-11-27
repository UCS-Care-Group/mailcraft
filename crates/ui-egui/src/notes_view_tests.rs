use super::notes_view::{self, Note};
use serde_json::json;

fn note(id: &str, modified: &str, is_deleted: bool) -> Note {
    let mut note = Note::default();
    note.id = id.to_string();
    note.modified_date = Some(modified.to_string());
    note.is_deleted = is_deleted;
    note
}

#[test]
fn a_stale_cloud_note_does_not_resurrect_a_deleted_note() {
    let mut local = vec![note("n1", "2026-11-27T10:00:00Z", true)];
    let cloud = vec![note("n1", "2026-11-27T09:00:00Z", false)];

    notes_view::smart_merge_notes(&mut local, cloud);

    assert!(local[0].is_deleted);
}

#[test]
fn an_ai_reply_is_not_appended_again_after_html_line_breaks_are_saved() {
    let reply = "Primeira linha\nSegunda linha";
    let saved_html = "<div>Texto original</div><br/><br/><div><b>🤖 Resposta do Assistente IA:</b><br/>Primeira linha<br/>Segunda linha</div>";

    assert!(notes_view::ai_reply_already_embedded(saved_html, reply));
}

#[test]
fn gemini_input_and_send_button_fit_the_available_width() {
    for available in [120.0, 200.0, 800.0] {
        let (input, button, spacing) = notes_view::gemini_input_layout(available, 8.0);
        assert!(input + button + spacing <= available);
    }
}

#[test]
fn a_reply_is_attached_to_the_note_that_requested_it() {
    let mut notes = vec![note("n1", "2026-11-27T10:00:00Z", false), note("n2", "2026-11-27T10:00:00Z", false)];
    notes[0].gemini_chat = Some(json!([{"role":"ai","text":"Pensando..."}]));
    notes[1].gemini_chat = Some(json!([{"role":"ai","text":"Pensando..."}]));

    notes_view::update_gemini_ai_message(&mut notes, "n1", "Resposta de n1");

    assert_eq!(notes[0].gemini_chat.as_ref().and_then(|v| v[0]["text"].as_str()), Some("Resposta de n1"));
    assert_eq!(notes[1].gemini_chat.as_ref().and_then(|v| v[0]["text"].as_str()), Some("Pensando..."));
}
