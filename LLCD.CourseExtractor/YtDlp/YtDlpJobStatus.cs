namespace LLCD.CourseExtractor.YtDlp
{
    public enum YtDlpJobStatus
    {
        Queued,
        FetchingInfo,
        Ready,
        Downloading,
        Converting,
        Finished,
        Failed,
        Cancelled
    }
}
