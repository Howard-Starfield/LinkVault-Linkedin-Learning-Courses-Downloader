namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpProgress
    {
        public YtDlpJobStatus Status { get; set; }

        public double? Percent { get; set; }

        public string TotalSize { get; set; }

        public string Speed { get; set; }

        public string Eta { get; set; }

        public string Message { get; set; }

        public string FilePath { get; set; }

        public string RawLine { get; set; }
    }
}
