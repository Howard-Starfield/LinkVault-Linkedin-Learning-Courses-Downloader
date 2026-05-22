namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpDependencyInstallProgress
    {
        public string Message { get; set; }

        public string CurrentFile { get; set; }

        public long BytesReceived { get; set; }

        public long? TotalBytes { get; set; }

        public double? Percent { get; set; }
    }
}
