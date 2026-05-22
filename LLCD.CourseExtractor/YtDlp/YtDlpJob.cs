using System;
using System.Collections.Generic;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpJob
    {
        public YtDlpJob()
        {
            Id = Guid.NewGuid().ToString("N");
            CreatedAt = DateTimeOffset.UtcNow;
            Status = YtDlpJobStatus.Queued;
            Logs = new List<string>();
        }

        public string Id { get; set; }

        public string Url { get; set; }

        public string Title { get; set; }

        public string OutputTemplate { get; set; }

        public string OutputFilePath { get; set; }

        public string OutputFileName { get; set; }

        public YtDlpDownloadOptions Options { get; set; }

        public YtDlpInfo Info { get; set; }

        public YtDlpJobStatus Status { get; set; }

        public YtDlpProgress Progress { get; set; }

        public string ErrorMessage { get; set; }

        public DateTimeOffset CreatedAt { get; set; }

        public DateTimeOffset? StartedAt { get; set; }

        public DateTimeOffset? CompletedAt { get; set; }

        public List<string> Logs { get; }

        public void ResetForRetry()
        {
            Status = YtDlpJobStatus.Queued;
            Progress = null;
            ErrorMessage = null;
            OutputFilePath = null;
            OutputFileName = null;
            StartedAt = null;
            CompletedAt = null;
            Logs.Clear();
        }
    }
}
