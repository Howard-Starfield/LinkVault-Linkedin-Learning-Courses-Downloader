namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpDownloadResult
    {
        public bool Success { get; set; }

        public int ExitCode { get; set; }

        public string FilePath { get; set; }

        public string FileName { get; set; }

        public string Error { get; set; }

        public string StandardOutput { get; set; }

        public string StandardError { get; set; }
    }
}
