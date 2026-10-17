//! 技术参数探测（ffprobe）。
//!
//! ## 只读，绝不写入
//! ffprobe 是**只读**的解析器：它打开视频文件读取容器头与流信息，然后退出。
//! 本模块调用它时不会传任何输出/写入参数，也不会以写模式打开媒体文件；
//! 探测结果只写进数据库的 `tech_json` 字段。视频文件本身一字不动。
//!
//! ## 为什么默认不参与类型判定
//! 一旦把 `duration` 喂给 `Signals.duration_secs`，R11 就会生效：
//! `duration ≥ 100 分钟` 会给 `movie + 2`，而 `movie_threshold = 2`
//! ——**足以把一批 Unknown 直接翻成 Movie**，正是 `mediatype.rs` 明文要避免的。
//! 所以本期只填字段，R11 继续不生效；是否启用要等一次离线翻案统计再裁决。
//!
//! ## 接线状态：**已实现，尚未接入扫描流程**（刻意）
//! 接入它会让类型判定结果发生变化（见上一节）。这是**行为变更**，
//! 需要一个单独观察窗口，不能混在其它改动里悄悄生效。
//! 在接入之前，本模块的公开项都不会被引用到 —— 下面的 `allow` 是**刻意保留**，
//! 不是遗漏；参见 `docs/v6-开发记录-2026-10-04.md` §五/§七。
#![allow(dead_code)]

use crate::util::no_console;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// 探测结果。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TechInfo {
    pub duration_secs: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: Option<String>,
    /// 原始 JSON（留档，便于以后加字段不用重新探测）。
    pub raw: Option<String>,
}

/// ffprobe 能不能用（没装就返回 false，调用方应跳过而不是报错）。
pub fn available() -> bool {
    find_ffprobe().is_some()
}

/// 找 ffprobe：先看环境变量 `FFPROBE`，再找常见位置。
pub fn find_ffprobe() -> Option<String> {
    if let Ok(p) = std::env::var("FFPROBE") {
        if !p.trim().is_empty() && Path::new(&p).is_file() {
            return Some(p);
        }
    }
    // PATH 里找。ffprobe 是控制台程序，不带 CREATE_NO_WINDOW 时，GUI 主进程每探一次
    // 就会被系统塞一个控制台窗口（扫描入库时表现为黑框连闪）。
    let out = no_console(Command::new("ffprobe").arg("-version")).output();
    if out.is_ok() {
        return Some("ffprobe".to_string());
    }
    for c in [
        r"C:\Program Files\ffmpeg\bin\ffprobe.exe",
        r"C:\ffmpeg\bin\ffprobe.exe",
        r"D:\ffmpeg\bin\ffprobe.exe",
    ] {
        if Path::new(c).is_file() {
            return Some(c.to_string());
        }
    }
    None
}

/// 探测一个文件（**只读**）。
///
/// 失败一律返回 `Err`，调用方据此跳过——探测失败不能影响入库主流程。
pub fn probe(path: &str) -> Result<TechInfo, String> {
    if is_video_file(path) == false {
        return Err("拒绝探测非视频文件".to_string());
    }
    let exe = find_ffprobe().ok_or_else(|| "未找到 ffprobe".to_string())?;
    // 注意：这里只有 -show_streams/-show_format 这类**读**参数，没有任何输出重定向；
    // `no_console` 抑制 ffprobe 的控制台窗口（同上，避免扫描时闪黑框）。
    let out = no_console(Command::new(&exe).args([
        "-v", "quiet",
        "-print_format", "json",
        "-show_format",
        "-show_streams",
        path,
    ]))
    .output()
    .map_err(|e| format!("调用 ffprobe 失败：{e}"))?;
    if !out.status.success() {
        return Err("ffprobe 解析失败".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    parse_ffprobe_json(&text).ok_or_else(|| "ffprobe 输出无法解析".to_string())
}

/// 从 ffprobe 的 JSON 里抠字段（不引第三方 JSON 库，够用就好）。
pub fn parse_ffprobe_json(text: &str) -> Option<TechInfo> {
    let mut info = TechInfo::default();
    // 时长：format.duration
    info.duration_secs = json_number_after(text, "\"duration\"").and_then(|f| {
        let secs = f as i64;
        if secs > 0 { Some(secs) } else { None }
    });
    info.container = json_string_after(text, "\"format_name\"");
    // 分辨率 / 编码：取第一条视频流
    if let Some(video) = first_video_stream(text) {
        info.width = json_number_after(video, "\"width\"").map(|n| n as i64);
        info.height = json_number_after(video, "\"height\"").map(|n| n as i64);
        info.video_codec = json_string_after(video, "\"codec_name\"");
    }
    if let Some(audio) = first_audio_stream(text) {
        info.audio_codec = json_string_after(audio, "\"codec_name\"");
    }
    info.raw = Some(text.to_string());
    Some(info)
}

/// 分辨率文本（`1920x1080`），没有就 None。
pub fn resolution_text(info: &TechInfo) -> Option<String> {
    match (info.width, info.height) {
        (Some(w), Some(h)) => Some(format!("{w}x{h}")),
        _ => None,
    }
}

fn is_video_file(p: &str) -> bool {
    // 复用 `core` 的判定（provider 复用 core 的数据类型与纯判定是被允许的，
    // 反方向——core 依赖 provider——才是禁止的）。
    crate::core::parser::is_video_file(p)
}

/// 找 `"key"` 之后的第一个数字。
fn json_number_after(s: &str, key: &str) -> Option<f64> {
    let i = s.find(key)?;
    let rest = &s[i + key.len()..];
    let start = rest.find(|c: char| c.is_ascii_digit() || c == '-')?;
    let rest = &rest[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}

/// 找 `"key"` 之后的第一个字符串值。
fn json_string_after(s: &str, key: &str) -> Option<String> {
    let i = s.find(key)?;
    let rest = &s[i + key.len()..];
    let start = rest.find('"')? + 1;
    let rest = &rest[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 切出第一段 `codec_type":"video` 所在的流对象（粗略但够用）。
fn first_stream_of<'a>(s: &'a str, kind: &str) -> Option<&'a str> {
    let marker = format!("\"codec_type\":\"{kind}\"");
    let idx = s.find(&marker)?;
    // 往前找这个流的开始 `{`
    let before = &s[..idx];
    let start = before.rfind('{')?;
    // 往后找这个流的结束 `}`（简单计数配对）
    let after = &s[idx..];
    let mut depth = 0i32;
    for (i, c) in after.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth <= 0 {
                    return Some(&s[start..idx + i + 1]);
                }
            }
            _ => {}
        }
    }
    Some(&s[start..])
}

fn first_video_stream(s: &str) -> Option<&str> {
    first_stream_of(s, "video")
}

fn first_audio_stream(s: &str) -> Option<&str> {
    first_stream_of(s, "audio")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "streams": [
    {"index":0,"codec_name":"h264","codec_type":"video","width":1920,"height":1080},
    {"index":1,"codec_name":"aac","codec_type":"audio"}
  ],
  "format": {"format_name":"matroska,webm","duration":"7245.123000"}
}"#;

    #[test]
    fn parses_duration_and_container() {
        let info = parse_ffprobe_json(SAMPLE).unwrap();
        assert_eq!(info.duration_secs, Some(7245));
        assert_eq!(info.container.as_deref(), Some("matroska,webm"));
    }

    #[test]
    fn parses_video_and_audio_streams() {
        let info = parse_ffprobe_json(SAMPLE).unwrap();
        assert_eq!(info.width, Some(1920));
        assert_eq!(info.height, Some(1080));
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.audio_codec.as_deref(), Some("aac"));
        assert_eq!(resolution_text(&info).as_deref(), Some("1920x1080"));
    }

    /// 音频流先出现时也必须能正确定位视频流（顺序不保证）。
    #[test]
    fn finds_video_stream_regardless_of_order() {
        let s = r#"{"streams":[{"codec_name":"aac","codec_type":"audio"},
                    {"codec_name":"hevc","codec_type":"video","width":3840,"height":2160}]}"#;
        let info = parse_ffprobe_json(s).unwrap();
        assert_eq!(info.video_codec.as_deref(), Some("hevc"));
        assert_eq!(info.width, Some(3840));
    }

    /// 绝不探测非视频文件——这是「不碰无关文件」的兜底。
    #[test]
    fn refuses_to_probe_non_video() {
        assert!(probe("a/b/c.nfo").is_err());
        assert!(probe("a/b/c.txt").is_err());
    }

    #[test]
    fn garbage_json_is_tolerated() {
        let info = parse_ffprobe_json("not json");
        assert!(info.is_some(), "解析失败应返回 None 或默认结构，不能 panic");
        let i = info.unwrap();
        assert!(i.width.is_none());
        assert!(i.duration_secs.is_none());
    }
}
