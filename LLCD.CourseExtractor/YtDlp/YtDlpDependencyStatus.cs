namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpDependencyStatus
    {
        public string YtDlpPath { get; set; }

        public string FfmpegPath { get; set; }

        public string FfprobePath { get; set; }

        public bool HasYtDlp => !string.IsNullOrWhiteSpace(YtDlpPath);

        public bool HasFfmpeg => !string.IsNullOrWhiteSpace(FfmpegPath);

        public bool HasFfprobe => !string.IsNullOrWhiteSpace(FfprobePath);

        public bool CanFetchMetadata => HasYtDlp;

        public bool CanDownloadVideo => HasYtDlp && HasFfmpeg;

        public bool CanMergeOrExtractAudio => HasYtDlp && HasFfmpeg;

        public bool CanRun(YtDlpDownloadOptions options)
        {
            if (!HasYtDlp)
                return false;

            return options == null || !options.RequiresFfmpeg || HasFfmpeg;
        }
    }
}
