namespace LLCD.CourseExtractor.YtDlp
{
    public static class YtDlpDependencyChecker
    {
        public static YtDlpDependencyStatus Check(string appRootDirectory = null)
        {
            return new YtDlpDependencyStatus
            {
                YtDlpPath = YtDlpToolLocator.FindYtDlp(appRootDirectory),
                FfmpegPath = YtDlpToolLocator.FindFfmpeg(appRootDirectory),
                FfprobePath = YtDlpToolLocator.FindFfprobe(appRootDirectory)
            };
        }
    }
}
