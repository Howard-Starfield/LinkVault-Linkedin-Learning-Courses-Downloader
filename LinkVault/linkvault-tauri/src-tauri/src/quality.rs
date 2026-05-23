use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
pub enum VideoQuality {
    #[serde(rename = "1080")]
    P1080,
    #[serde(rename = "720")]
    P720,
    #[serde(rename = "540")]
    P540,
    #[serde(rename = "360")]
    P360,
}

pub fn fallback_order(selected: VideoQuality) -> Vec<VideoQuality> {
    let ordered = [
        VideoQuality::P1080,
        VideoQuality::P720,
        VideoQuality::P540,
        VideoQuality::P360,
    ];
    let start = ordered
        .iter()
        .position(|quality| *quality == selected)
        .unwrap_or(0);
    ordered[start..].to_vec()
}

pub fn choose_best_available(
    selected: VideoQuality,
    available: &[VideoQuality],
) -> Option<VideoQuality> {
    fallback_order(selected)
        .into_iter()
        .find(|candidate| available.contains(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_1080_falls_back_to_720_540_360() {
        assert_eq!(
            fallback_order(VideoQuality::P1080),
            vec![
                VideoQuality::P1080,
                VideoQuality::P720,
                VideoQuality::P540,
                VideoQuality::P360
            ]
        );
    }

    #[test]
    fn picks_720_when_1080_is_unavailable() {
        let available = [VideoQuality::P720, VideoQuality::P360];

        assert_eq!(
            choose_best_available(VideoQuality::P1080, &available),
            Some(VideoQuality::P720)
        );
    }
}
