use crate::usage::dates::parse_iso8601_ms;
use crate::usage::types::TokenCounts;
use serde::Deserialize;

/// Model id that carries no API cost. Observed 52 times, always with
/// isApiErrorMessage:true and all-zero usage.
const SYNTHETIC_MODEL: &str = "<synthetic>";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RecordKind {
    Assistant,
    User,
    Other,
}

#[derive(Debug, Clone)]
pub struct ParsedRecord {
    pub kind: RecordKind,
    pub ts_ms: i64,
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub message_id: Option<String>,
    pub model: Option<String>,
    /// Record uuid. Dedup key for user messages (72,443 raw vs 57,122 distinct).
    pub uuid: Option<String>,
    /// ids of tool_use blocks on this line. Dedup key for tool calls: each
    /// line carries exactly ONE content block, so message.id cannot serve.
    pub tool_use_ids: Vec<String>,
    /// True when content is a non-empty array of only `tool_result` blocks:
    /// a tool-result echo, not a human message.
    pub is_tool_result_echo: bool,
    /// None for user/other records, for assistant records with no usage
    /// block, and for `<synthetic>` models.
    pub usage: Option<TokenCounts>,
}

#[derive(Debug)]
pub enum ParseOutcome {
    Record(ParsedRecord),
    /// Blank line, or a record we deliberately ignore. Not an error.
    Skipped,
    /// Unparseable. Counted and surfaced as a diagnostic.
    Malformed,
}

// --- Wire types: every field optional so a missing key skips, never fails ---

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
    #[serde(rename = "attributionAgent")]
    attribution_agent: Option<String>,
    uuid: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    /// User prompts carry a STRING here; assistant records carry an array.
    /// Untagged so either shape parses instead of failing the whole line.
    #[serde(default)]
    content: Option<RawContent>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawContent {
    Text(String),
    Blocks(Vec<RawBlock>),
}

#[derive(Deserialize)]
struct RawBlock {
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Present on every observed tool_use block.
    id: Option<String>,
}

/// Every count is Option: `#[serde(default)]` alone tolerates a MISSING key
/// but an explicit `null` would still fail the whole line, contradicting the
/// "missing field means skip the record, never fail" constraint.
#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<RawCacheCreation>,
    output_tokens_details: Option<RawOutputDetails>,
    server_tool_use: Option<RawServerToolUse>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<u64>,
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct RawOutputDetails {
    #[serde(default)]
    thinking_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct RawServerToolUse {
    #[serde(default)]
    web_search_requests: Option<u64>,
    #[serde(default)]
    web_fetch_requests: Option<u64>,
}

impl RawUsage {
    fn to_counts(&self) -> TokenCounts {
        TokenCounts {
            input: self.input_tokens.unwrap_or(0),
            output: self.output_tokens.unwrap_or(0),
            cache_read: self.cache_read_input_tokens.unwrap_or(0),
            cache_creation: self.cache_creation_input_tokens.unwrap_or(0),
            cache_1h: self
                .cache_creation
                .as_ref()
                .and_then(|c| c.ephemeral_1h_input_tokens)
                .unwrap_or(0),
            cache_5m: self
                .cache_creation
                .as_ref()
                .and_then(|c| c.ephemeral_5m_input_tokens)
                .unwrap_or(0),
            thinking: self
                .output_tokens_details
                .as_ref()
                .and_then(|d| d.thinking_tokens)
                .unwrap_or(0),
            web_search: self
                .server_tool_use
                .as_ref()
                .and_then(|s| s.web_search_requests)
                .unwrap_or(0),
            web_fetch: self
                .server_tool_use
                .as_ref()
                .and_then(|s| s.web_fetch_requests)
                .unwrap_or(0),
        }
    }
}

pub fn parse_line(line: &str) -> ParseOutcome {
    if line.trim().is_empty() {
        return ParseOutcome::Skipped;
    }
    let raw: RawLine = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(_) => return ParseOutcome::Malformed,
    };

    let ts_ms = match raw.timestamp.as_deref().and_then(parse_iso8601_ms) {
        Some(v) => v,
        None => return ParseOutcome::Skipped,
    };
    let session_id = match raw.session_id {
        Some(s) => s,
        None => return ParseOutcome::Skipped,
    };

    let kind = match raw.kind.as_deref() {
        Some("assistant") => RecordKind::Assistant,
        Some("user") => RecordKind::User,
        _ => RecordKind::Other,
    };

    let msg = raw.message;
    let model = msg.as_ref().and_then(|m| m.model.clone());
    let message_id = msg.as_ref().and_then(|m| m.id.clone());

    let blocks: &[RawBlock] = match msg.as_ref().and_then(|m| m.content.as_ref()) {
        Some(RawContent::Blocks(b)) => b.as_slice(),
        _ => &[],
    };

    let tool_use_ids: Vec<String> = blocks
        .iter()
        .filter(|b| b.kind.as_deref() == Some("tool_use"))
        .filter_map(|b| b.id.clone())
        .collect();

    let is_tool_result_echo = !blocks.is_empty()
        && blocks
            .iter()
            .all(|b| b.kind.as_deref() == Some("tool_result"));

    let usage = if kind == RecordKind::Assistant && model.as_deref() != Some(SYNTHETIC_MODEL) {
        msg.as_ref().and_then(|m| m.usage.as_ref()).map(|u| u.to_counts())
    } else {
        None
    };

    ParseOutcome::Record(ParsedRecord {
        kind,
        ts_ms,
        session_id,
        cwd: raw.cwd,
        git_branch: raw.git_branch,
        agent_id: raw.agent_id,
        agent_type: raw.attribution_agent,
        message_id,
        model,
        uuid: raw.uuid,
        tool_use_ids,
        is_tool_result_echo,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSISTANT: &str = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59.076Z","sessionId":"s-1","cwd":"/Users/me/proj","gitBranch":"main","requestId":"req_1","apiBlockIndex":0,"message":{"id":"msg_1","model":"claude-opus-5","role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":2,"output_tokens":894,"cache_read_input_tokens":48669,"cache_creation_input_tokens":2722,"cache_creation":{"ephemeral_1h_input_tokens":2722,"ephemeral_5m_input_tokens":0},"output_tokens_details":{"thinking_tokens":300},"server_tool_use":{"web_search_requests":1,"web_fetch_requests":2}}}}"#;

    #[test]
    fn parses_assistant_usage_into_token_counts() {
        let rec = match parse_line(ASSISTANT) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.kind, RecordKind::Assistant);
        assert_eq!(rec.session_id, "s-1");
        assert_eq!(rec.message_id.as_deref(), Some("msg_1"));
        assert_eq!(rec.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(rec.cwd.as_deref(), Some("/Users/me/proj"));
        assert_eq!(rec.git_branch.as_deref(), Some("main"));
        assert_eq!(rec.ts_ms, 1789029179076);

        let u = rec.usage.expect("usage");
        assert_eq!(u.input, 2);
        assert_eq!(u.output, 894);
        assert_eq!(u.cache_read, 48669);
        assert_eq!(u.cache_creation, 2722);
        assert_eq!(u.cache_1h, 2722);
        assert_eq!(u.cache_5m, 0);
        assert_eq!(u.thinking, 300);
        assert_eq!(u.web_search, 1);
        assert_eq!(u.web_fetch, 2);
    }

    #[test]
    fn collects_tool_use_block_ids_for_dedup() {
        // Real tool_use blocks always carry an id (0 missing in 68,492
        // observed blocks); the id is the only safe tool-call dedup key.
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s","message":{"id":"m","model":"claude-opus-5","content":[{"type":"text","text":"a"},{"type":"tool_use","id":"toolu_a","name":"Bash"},{"type":"tool_use","id":"toolu_b","name":"Read"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.tool_use_ids, vec!["toolu_a".to_string(), "toolu_b".to_string()]);
    }

    #[test]
    fn captures_user_record_uuid_for_dedup() {
        let line = r#"{"type":"user","uuid":"u-1","timestamp":"2026-09-10T08:30:00Z","sessionId":"s","message":{"role":"user","content":"hello"}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.uuid.as_deref(), Some("u-1"));
    }

    #[test]
    fn excludes_synthetic_model_usage_but_keeps_the_record() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s","isApiErrorMessage":true,"message":{"id":"m","model":"<synthetic>","content":[],"usage":{"input_tokens":0,"output_tokens":0}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert!(rec.usage.is_none(), "synthetic usage must not be counted");
        assert_eq!(rec.model.as_deref(), Some("<synthetic>"));
    }

    #[test]
    fn parses_subagent_attribution_fields() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"parent-s","agentId":"a05e8f2db40a521f0","attributionAgent":"general-purpose","isSidechain":true,"message":{"id":"m","model":"claude-sonnet-5","content":[],"usage":{"input_tokens":1,"output_tokens":2}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        // Subagent records share the PARENT's sessionId.
        assert_eq!(rec.session_id, "parent-s");
        assert_eq!(rec.agent_id.as_deref(), Some("a05e8f2db40a521f0"));
        assert_eq!(rec.agent_type.as_deref(), Some("general-purpose"));
    }

    #[test]
    fn subagent_without_attribution_agent_yields_none() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"p","agentId":"a1","message":{"id":"m","model":"claude-sonnet-5","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.agent_id.as_deref(), Some("a1"));
        assert_eq!(rec.agent_type, None);
    }

    #[test]
    fn assistant_record_missing_usage_parses_with_no_usage() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s","message":{"id":"m","model":"claude-opus-5","content":[]}}"#;
        match parse_line(line) {
            ParseOutcome::Record(r) => assert!(r.usage.is_none()),
            other => panic!("expected Record, got {:?}", other),
        }
    }

    #[test]
    fn parses_user_record_with_string_content() {
        // Every typed human prompt has `content` as a STRING, not an array
        // (2,243 observed). A Vec-only wire type rejects all of them.
        let line = r#"{"type":"user","timestamp":"2026-09-10T08:30:00Z","sessionId":"s","message":{"role":"user","content":"hello"}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.kind, RecordKind::User);
        assert!(rec.usage.is_none());
        assert!(!rec.is_tool_result_echo);
    }

    #[test]
    fn flags_a_user_record_that_is_only_tool_results() {
        // 68,600 of 70,308 array-content user records are tool-result echoes.
        // They are transport, not messages.
        let line = r#"{"type":"user","timestamp":"2026-09-10T08:30:00Z","sessionId":"s","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_a"}]}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert!(rec.is_tool_result_echo);
    }

    #[test]
    fn a_user_record_mixing_text_with_tool_results_is_a_real_message() {
        let line = r#"{"type":"user","timestamp":"2026-09-10T08:30:00Z","sessionId":"s","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t"},{"type":"text","text":"and also"}]}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert!(!rec.is_tool_result_echo);
    }

    #[test]
    fn other_record_types_parse_as_other_for_activity_timestamps() {
        // These still matter: last_ts must reflect ANY record so a session
        // mid-Bash-call is not reported as idle.
        let line = r#"{"type":"file-history-snapshot","timestamp":"2026-09-10T08:40:00Z","sessionId":"s"}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.kind, RecordKind::Other);
    }

    #[test]
    fn malformed_json_is_reported_not_panicked() {
        assert!(matches!(parse_line("{not json"), ParseOutcome::Malformed));
        assert!(matches!(parse_line(""), ParseOutcome::Skipped));
        assert!(matches!(parse_line("   "), ParseOutcome::Skipped));
    }

    #[test]
    fn record_without_timestamp_or_session_is_skipped_not_fatal() {
        let no_ts = r#"{"type":"assistant","sessionId":"s","message":{"id":"m"}}"#;
        assert!(matches!(parse_line(no_ts), ParseOutcome::Skipped));
        let no_session = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","message":{"id":"m"}}"#;
        assert!(matches!(parse_line(no_session), ParseOutcome::Skipped));
    }

}
