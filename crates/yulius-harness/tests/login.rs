//! Login flow contracts: token capture and streaming masking (SPEC-002 §7.2 "Login", §8,
//! AC-NFR-7). All tokens are synthetic.

use yulius_harness::login::{
    MASK, TokenMasker, claude_setup_token_argv, extract_claude_oauth_token,
};

const TOKEN: &str = "sk-ant-oat01-SYNTH_abc-DEF0123456789xyzSYNTHETICtoken";

fn mask_in_chunks(input: &[u8], cuts: &[usize]) -> Vec<u8> {
    let mut masker = TokenMasker::new();
    let mut out = Vec::new();
    let mut start = 0;
    for &cut in cuts {
        out.extend(masker.push(&input[start..cut]));
        start = cut;
    }
    out.extend(masker.push(&input[start..]));
    out.extend(masker.finish());
    out
}

#[test]
fn argv_of_the_login_command() {
    assert_eq!(claude_setup_token_argv(), ["claude", "setup-token"]);
}

#[test]
fn extracts_token_from_tui_output() {
    let output = format!(
        "\x1b[32m✓ Long-lived token created\x1b[0m\r\n\x1b[1m{TOKEN}\x1b[0m\r\nStore it securely.\r\n"
    );
    assert_eq!(extract_claude_oauth_token(&output).as_deref(), Some(TOKEN));
    let twice = format!("old sk-ant-oat01-{} then {TOKEN}", "A".repeat(25));
    assert_eq!(extract_claude_oauth_token(&twice).as_deref(), Some(TOKEN));
    assert_eq!(extract_claude_oauth_token("sk-ant-oat01-short"), None);
    assert_eq!(extract_claude_oauth_token("no token here"), None);
    assert_eq!(
        extract_claude_oauth_token(&format!("sk-ant-oat01-{}", "B".repeat(20))).map(|t| t.len()),
        Some(33)
    );
}

#[test]
fn masks_token_split_at_every_position() {
    let input = format!("before {TOKEN} after\n").into_bytes();
    let expected = format!("before {MASK} after\n").into_bytes();
    for cut in 0..=input.len() {
        assert_eq!(mask_in_chunks(&input, &[cut]), expected, "cut at {cut}");
    }
    // Byte-by-byte delivery.
    let all_cuts: Vec<usize> = (1..input.len()).collect();
    assert_eq!(mask_in_chunks(&input, &all_cuts), expected);
}

#[test]
fn masks_multiple_tokens_and_passes_other_text_unchanged() {
    let input = format!("{TOKEN}\n{TOKEN}").into_bytes();
    assert_eq!(
        mask_in_chunks(&input, &[5, 40]),
        format!("{MASK}\n{MASK}").into_bytes()
    );
    let plain = "sk-ant-api03-not-an-oauth-token and sk-ant-oat01-short.\n".as_bytes();
    assert_eq!(mask_in_chunks(plain, &[3, 20, 41]), plain);
    let unicode = "zażółć gęślą jaźń\n".as_bytes();
    assert_eq!(mask_in_chunks(unicode, &[1, 2, 3]), unicode);
}

#[test]
fn masker_holds_back_only_a_bounded_tail() {
    let mut masker = TokenMasker::new();
    let first = masker.push(b"hello sk-ant-oat01-ABCDEFGHIJ");
    assert_eq!(first, b"hello ");
    let second = masker.push(b"KLMNOPQRST and more");
    assert_eq!(second, format!("{MASK} and more").into_bytes());
    // A token reaching the end of the stream with fewer than 20 characters is not a token.
    let mut short = TokenMasker::new();
    assert_eq!(short.push(b"sk-ant-oat01-ABC"), b"");
    assert_eq!(short.finish(), b"sk-ant-oat01-ABC");
}
