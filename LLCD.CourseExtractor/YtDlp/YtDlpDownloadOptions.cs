namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpDownloadOptions
    {
        public string Url { get; set; }

        public string OutputTemplate { get; set; }

        public string FfmpegLocation { get; set; }

        public YtDlpFormatChoice FormatChoice { get; set; } = YtDlpFormatChoice.Video;

        public string FormatId { get; set; }

        public YtDlpAudioFormat AudioFormat { get; set; } = YtDlpAudioFormat.Mp3;

        public bool WriteSubtitles { get; set; }

        public bool WriteAutomaticSubtitles { get; set; }

        public string SubtitleLanguages { get; set; } = "en";

        public bool ConvertSubtitlesToSrt { get; set; } = true;

        public bool WriteInfoJson { get; set; }

        public bool WriteThumbnail { get; set; }

        public YtDlpBrowserCookiesSource CookiesSource { get; set; } = YtDlpBrowserCookiesSource.None;

        public bool NoPlaylist { get; set; } = true;

        public bool RequiresFfmpeg => FormatChoice == YtDlpFormatChoice.Audio || FormatChoice == YtDlpFormatChoice.Video;
    }
}
