//! Questions from the model to the user. The tool `miniclaw_ask` carries 1 to 4 questions, each
//! with 2 to 4 options, one pick or several, and the user may always write their own answer.
//! The answers go back to the model as the tool's result. Asking grants nothing: the tool can
//! only put a question in front of the user and wait.

// Reads the model's questions and the user's answers.
use serde::{Deserialize, Serialize};
// Builds the tool's definition.
use serde_json::{Value, json};

/// The tool's name, as both providers see it.
pub const TOOL: &str = "miniclaw_ask";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub label: String,
    #[serde(default)] pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Question {
    pub question: String,
    #[serde(default)] pub header: String,
    #[serde(default)] pub multi_select: bool,
    pub options: Vec<Choice>,
}

/// The user's answer to one question: the options they picked and, if any, their own words.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    #[serde(default)] pub selected: Vec<String>,
    #[serde(default)] pub other: Option<String>,
}

/// Accepts one line of printable text up to a length.
fn line(text: &str, most: usize) -> bool {
    !text.trim().is_empty() && text.len() <= most && !text.chars().any(|c| c.is_control() && c != '\n')
}

/// Reads the model's arguments. Anything outside the shape is refused as a whole.
pub fn parse(arguments: &Value) -> Option<Vec<Question>> {
    let questions: Vec<Question> = serde_json::from_value(arguments.get("questions")?.clone()).ok()?;
    let labels_differ = |question: &Question| {
        let mut labels: Vec<&str> = question.options.iter().map(|option| option.label.as_str()).collect();
        labels.sort_unstable();
        labels.windows(2).all(|pair| pair[0] != pair[1])
    };
    let valid = (1..=4).contains(&questions.len()) && questions.iter().all(|question| line(&question.question, 600)
        && question.header.len() <= 40 && !question.header.chars().any(char::is_control)
        && (2..=4).contains(&question.options.len()) && labels_differ(question)
        && question.options.iter().all(|option| line(&option.label, 80) && option.description.len() <= 300
            && !option.description.chars().any(char::is_control)));
    valid.then_some(questions)
}

/// Checks the user's answers against the questions: one answer each, only offered labels, one
/// pick unless several are allowed, and every question answered by a pick or by their own words.
pub fn answers_fit(questions: &[Question], answers: &[Answer]) -> bool {
    questions.len() == answers.len() && questions.iter().zip(answers).all(|(question, answer)| {
        let own = answer.other.as_deref().map(str::trim).filter(|text| !text.is_empty());
        answer.selected.iter().all(|label| question.options.iter().any(|option| &option.label == label))
            && answer.selected.iter().collect::<std::collections::HashSet<_>>().len() == answer.selected.len()
            && (question.multi_select || answer.selected.len() <= 1)
            && (!answer.selected.is_empty() || own.is_some())
            && own.is_none_or(|text| text.len() <= 4000 && !text.contains('\0'))
    })
}

/// Writes the answers for the model: each question followed by what the user chose and wrote.
pub fn result_text(questions: &[Question], answers: &[Answer]) -> String {
    let mut text = String::from("The user answered:\n");
    for (question, answer) in questions.iter().zip(answers) {
        text.push_str(&format!("\nQ: {}\nA: ", question.question));
        let own = answer.other.as_deref().map(str::trim).filter(|own| !own.is_empty());
        match (answer.selected.is_empty(), own) {
            (false, Some(own)) => text.push_str(&format!("{}. In their own words: {own}", answer.selected.join(", "))),
            (false, None) => text.push_str(&answer.selected.join(", ")),
            (true, Some(own)) => text.push_str(&format!("In their own words: {own}")),
            (true, None) => text.push_str("(no answer)"),
        }
        text.push('\n');
    }
    text
}

/// Describes the tool to a provider. `type` is required by the real Codex.
pub fn tool_definition() -> Value {
    let option = json!({ "type": "object", "additionalProperties": false, "required": ["label"], "properties": {
        "label": { "type": "string", "description": "The choice, in 1 to 5 words." },
        "description": { "type": "string", "description": "What choosing it means, in one sentence." } } });
    let question = json!({ "type": "object", "additionalProperties": false, "required": ["question", "options"], "properties": {
        "question": { "type": "string", "description": "The complete question, ending with a question mark." },
        "header": { "type": "string", "description": "A label of at most 12 characters, such as Language." },
        "multiSelect": { "type": "boolean", "description": "True when several options may be chosen together." },
        "options": { "type": "array", "minItems": 2, "maxItems": 4, "items": option } } });
    json!({ "type": "function", "name": TOOL,
        "description": "Asks the user 1 to 4 questions in a popup and returns their answers. Use it whenever a decision is genuinely the user's and changes what you do next: a choice between approaches, a preference, a missing requirement, or a risky step you should confirm. Give each question 2 to 4 distinct options, and put the one you recommend first with \"(Recommended)\" at the end of its label. Do not add an option such as Other: the user can always write their own answer. Look first: never ask what you can find out yourself. What files exist, what a file says, what is installed, which branch is checked out, what today's date is, which tools or MCP servers you have and what is switched on are all answered by your tools or by the MiniClaw environment note. Ask only what only the user can know. Do not ask about choices with an obvious default; take the default and say so. The turn waits for the answer; if the user does not answer, the turn ends.",
        "inputSchema": { "type": "object", "additionalProperties": false, "required": ["questions"], "properties": {
            "questions": { "type": "array", "minItems": 1, "maxItems": 4, "items": question } } } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Question> {
        parse(&json!({ "questions": [{ "question": "Which language?", "options": [{ "label": "TypeScript" }, { "label": "Python" }] },
            { "question": "Which tools?", "multiSelect": true, "options": [{ "label": "Search" }, { "label": "Create" }] }] })).unwrap()
    }

    #[test]
    fn questions_outside_the_shape_are_refused() {
        assert_eq!(sample().len(), 2);
        for bad in [json!({}), json!({ "questions": [] }), json!({ "questions": [{ "question": "One option?", "options": [{ "label": "A" }] }] }),
            json!({ "questions": [{ "question": "Twice?", "options": [{ "label": "A" }, { "label": "A" }] }] }),
            json!({ "questions": [{ "question": "", "options": [{ "label": "A" }, { "label": "B" }] }] })] {
            assert!(parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn answers_must_fit_their_questions() {
        let questions = sample();
        let pick = |labels: &[&str], other: Option<&str>| Answer { selected: labels.iter().map(|label| (*label).to_owned()).collect(),
            other: other.map(str::to_owned) };
        assert!(answers_fit(&questions, &[pick(&["Python"], None), pick(&["Search", "Create"], Some("and export"))]));
        assert!(answers_fit(&questions, &[pick(&[], Some("Go")), pick(&["Search"], None)]));
        assert!(!answers_fit(&questions, &[pick(&["TypeScript", "Python"], None), pick(&["Search"], None)]));
        assert!(!answers_fit(&questions, &[pick(&["Rust"], None), pick(&["Search"], None)]));
        assert!(!answers_fit(&questions, &[pick(&[], Some("  ")), pick(&["Search"], None)]));
        assert!(!answers_fit(&questions, &[pick(&["Python"], None)]));
    }
}
