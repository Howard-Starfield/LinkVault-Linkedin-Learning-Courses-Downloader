namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpProcessResult
    {
        public int ExitCode { get; set; }

        public string StandardOutput { get; set; }

        public string StandardError { get; set; }

        public bool IsSuccess => ExitCode == 0;
    }
}
