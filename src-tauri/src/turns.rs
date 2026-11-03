// turns.rs
// 전사 조각(RawUtterance)들을 "말하기 턴" 단위로 묶는다.
//
// 왜 필요한가:
// 라이브 녹음은 RMS VAD가 짧은 쉼마다 오디오를 끊고, 업로드 파일도 Whisper가
// 구(phrase) 단위로 세그먼트를 나눈다. 그래서 "It's almost winter but it-" /
// "It hadn't came, so..." / "Maybe in December it would be." 처럼 한 번에 한
// 말이 여러 줄로 쪼개진다. 조각마다 LLM을 따로 부르면
//   - 미완성 문장 자체를 "오류"로 보고 억지로 완성시키거나 (until this year's → this year)
//   - 문맥이 없어 의미를 바꿔버리는 (it became more... → It got better)
// 애매한 피드백이 나온다. 그래서 피드백 전에 조각들을 턴으로 합친다.
//
// 원문 전문(.txt)은 이 모듈과 무관하게 조각 단위 그대로 저장된다.

use crate::session::RawUtterance;

/// 이전 조각과 이 시간(초) 이내면 같은 턴으로 합친다.
/// 라이브 세션의 타임스탬프는 "전사가 끝난 시각"이라 조각 길이 + 쉼이 섞여 있어
/// 너무 짧게 잡으면 한 문장도 못 합치고, 너무 길게 잡으면 상대방 말 사이의
/// 내 대답들까지 한 덩어리가 된다.
const MERGE_GAP_SECS: i64 = 7;

/// 한 턴이 무한정 길어지지 않도록 하는 상한 (LLM이 다루기 좋은 길이 유지)
const MAX_TURN_WORDS: usize = 60;
const MAX_TURN_SPAN_SECS: i64 = 45;

/// 이것들로만 이루어진 조각은 피드백 대상에서 제외 (맞장구/추임새)
const FILLER_WORDS: &[&str] = &[
    "uh", "um", "uhm", "hmm", "mm", "mhm", "mm-hmm", "uh-huh", "huh", "ah", "oh", "ohh", "ohhh",
    "yeah", "yes", "yep", "ok", "okay", "right", "so", "well", "hey", "wow", "maybe", "and",
    "but", "like", "i", "no",
];

/// `(laughs)`, `[BLANK_AUDIO]`, `(phone ringing)` 같은 Whisper 주석과
/// 줄 앞의 화자 전환 대시("- ")를 제거한 텍스트
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_start_matches(|c: char| c == '-' || c == '–' || c.is_whitespace())
        .trim()
        .to_string()
}

/// 추임새/맞장구뿐인 조각인지 ("Yeah, yeah.", "Mm-hmm, mm-hmm.", "Uh...")
pub fn is_filler_only(cleaned: &str) -> bool {
    cleaned
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .all(|w| FILLER_WORDS.contains(&w.as_str()))
}

/// "HH:MM:SS"(라이브) 또는 "MM:SS"(업로드) → 초
fn parse_timestamp(ts: &str) -> Option<i64> {
    let parts: Vec<i64> = ts
        .split(':')
        .map(|p| p.trim().parse::<i64>())
        .collect::<Result<_, _>>()
        .ok()?;
    match parts.as_slice() {
        [h, m, s] => Some(h * 3600 + m * 60 + s),
        [m, s] => Some(m * 60 + s),
        _ => None,
    }
}

/// 피드백 대상 턴 하나
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// 턴 첫 조각의 타임스탬프
    pub timestamp: String,
    /// 조각들을 이어 붙인 텍스트
    pub text: String,
}

/// 조각 목록 → 턴 목록
/// 잡음/추임새 조각은 버리고, 가까운 조각끼리는 이어 붙인다.
pub fn group_into_turns(raw: &[RawUtterance]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    // 현재 턴의 (시작 시각, 마지막 조각 시각)
    let mut cur_start: Option<i64> = None;
    let mut cur_last: Option<i64> = None;

    for r in raw {
        let cleaned = clean_text(&r.text);
        if cleaned.is_empty() || is_filler_only(&cleaned) {
            continue;
        }
        let t = parse_timestamp(&r.timestamp);

        let can_merge = match (turns.last(), cur_start, cur_last, t) {
            (Some(last), Some(start), Some(prev), Some(now)) => {
                let gap = now - prev;
                let words = last.text.split_whitespace().count()
                    + cleaned.split_whitespace().count();
                (0..=MERGE_GAP_SECS).contains(&gap)
                    && now - start <= MAX_TURN_SPAN_SECS
                    && words <= MAX_TURN_WORDS
            }
            _ => false,
        };

        if can_merge {
            let last = turns.last_mut().expect("checked above");
            last.text.push(' ');
            last.text.push_str(&cleaned);
            cur_last = t;
        } else {
            turns.push(Turn {
                timestamp: r.timestamp.clone(),
                text: cleaned,
            });
            cur_start = t;
            cur_last = t;
        }
    }

    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(ts: &str, text: &str) -> RawUtterance {
        RawUtterance {
            timestamp: ts.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn strips_annotations_and_dashes() {
        assert_eq!(clean_text("(phone ringing)  Oh, hello."), "Oh, hello.");
        assert_eq!(clean_text("- That's a great part."), "That's a great part.");
        assert_eq!(clean_text("[BLANK_AUDIO]"), "");
        assert_eq!(clean_text("Ohhh. [laughs]"), "Ohhh.");
    }

    #[test]
    fn detects_fillers() {
        assert!(is_filler_only("Mm-hmm, mm-hmm, mm-hmm."));
        assert!(is_filler_only("Yeah, yeah."));
        assert!(is_filler_only("Uh..."));
        assert!(!is_filler_only("It hadn't came, so..."));
    }

    #[test]
    fn merges_close_fragments_into_one_turn() {
        let input = vec![
            raw("07:21:46", "- It's right, but."),
            raw("07:21:50", "It's almost winter but it-"),
            raw("07:21:51", "It hadn't came, so..."),
            raw("07:21:55", "Maybe in December it would be."),
            raw("07:22:14", "Mm-hmm. It didn't snow a lot"),
        ];
        let turns = group_into_turns(&input);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].timestamp, "07:21:46");
        assert_eq!(
            turns[0].text,
            "It's right, but. It's almost winter but it- It hadn't came, so... Maybe in December it would be."
        );
        assert_eq!(turns[1].text, "Mm-hmm. It didn't snow a lot");
    }

    #[test]
    fn drops_noise_without_breaking_merge() {
        let input = vec![
            raw("07:20:57", "(gentle music)"),
            raw("07:26:24", "about one week."),
            raw("07:26:27", "Before Christmas."),
            raw("07:26:28", "and after Christmas."),
            raw("07:26:36", "Oh."),
            raw("07:26:39", "There was a holidays."),
        ];
        let turns = group_into_turns(&input);
        assert_eq!(turns.len(), 2);
        assert_eq!(
            turns[0].text,
            "about one week. Before Christmas. and after Christmas."
        );
        assert_eq!(turns[1].text, "There was a holidays.");
    }

    #[test]
    fn works_with_upload_offsets() {
        let input = vec![raw("01:02", "I think climate has--"), raw("01:05", "changed a lot")];
        let turns = group_into_turns(&input);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "I think climate has-- changed a lot");
    }

    #[test]
    fn caps_turn_span() {
        let input: Vec<_> = (0..20)
            .map(|i| raw(&format!("00:{:02}", i * 5), "one two three"))
            .collect();
        let turns = group_into_turns(&input);
        assert!(turns.len() > 1);
    }
}
