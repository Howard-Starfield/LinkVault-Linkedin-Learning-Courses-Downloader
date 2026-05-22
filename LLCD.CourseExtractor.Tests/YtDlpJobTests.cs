using System;
using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpJobTests
    {
        [TestMethod]
        public void ResetForRetry_ClearsPreviousRunStateAndKeepsJobIdentity()
        {
            var createdAt = DateTimeOffset.UtcNow.AddMinutes(-10);
            var job = new YtDlpJob
            {
                Url = "https://example.com/video",
                Title = "Example",
                Status = YtDlpJobStatus.Failed,
                Progress = new YtDlpProgress { Percent = 42.5, Message = "Downloading" },
                ErrorMessage = "failed",
                OutputFilePath = @"D:\Videos\example.mp4",
                OutputFileName = "example.mp4",
                StartedAt = DateTimeOffset.UtcNow.AddMinutes(-5),
                CompletedAt = DateTimeOffset.UtcNow,
                CreatedAt = createdAt
            };
            var id = job.Id;
            job.Logs.Add("old log");

            job.ResetForRetry();

            Assert.AreEqual(id, job.Id);
            Assert.AreEqual(createdAt, job.CreatedAt);
            Assert.AreEqual("https://example.com/video", job.Url);
            Assert.AreEqual("Example", job.Title);
            Assert.AreEqual(YtDlpJobStatus.Queued, job.Status);
            Assert.IsNull(job.Progress);
            Assert.IsNull(job.ErrorMessage);
            Assert.IsNull(job.OutputFilePath);
            Assert.IsNull(job.OutputFileName);
            Assert.IsNull(job.StartedAt);
            Assert.IsNull(job.CompletedAt);
            Assert.AreEqual(0, job.Logs.Count);
        }
    }
}
