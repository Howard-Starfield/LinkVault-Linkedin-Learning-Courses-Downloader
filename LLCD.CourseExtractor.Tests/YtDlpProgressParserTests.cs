using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpProgressParserTests
    {
        [TestMethod]
        public void ParseLine_ForDownloadProgress_ReturnsPercentSpeedAndEta()
        {
            var progress = YtDlpProgressParser.ParseLine("[download]  12.3% of 10.00MiB at 1.23MiB/s ETA 00:12");

            Assert.IsNotNull(progress);
            Assert.AreEqual(YtDlpJobStatus.Downloading, progress.Status);
            Assert.AreEqual(12.3, progress.Percent.Value, 0.001);
            Assert.AreEqual("10.00MiB", progress.TotalSize);
            Assert.AreEqual("1.23MiB/s", progress.Speed);
            Assert.AreEqual("00:12", progress.Eta);
        }

        [TestMethod]
        public void ParseLine_ForUnknownSpeedProgress_ReturnsFullSpeedAndEta()
        {
            var progress = YtDlpProgressParser.ParseLine("[download]   0.1% of  967.79KiB at  Unknown B/s ETA Unknown");

            Assert.IsNotNull(progress);
            Assert.AreEqual(YtDlpJobStatus.Downloading, progress.Status);
            Assert.AreEqual(0.1, progress.Percent.Value, 0.001);
            Assert.AreEqual("967.79KiB", progress.TotalSize);
            Assert.AreEqual("Unknown B/s", progress.Speed);
            Assert.AreEqual("Unknown", progress.Eta);
        }

        [TestMethod]
        public void ParseLine_ForFinalDownloadLine_ReturnsPercentAndSize()
        {
            var progress = YtDlpProgressParser.ParseLine("[download] 100% of  967.79KiB in 00:00:00 at 5.58MiB/s");

            Assert.IsNotNull(progress);
            Assert.AreEqual(YtDlpJobStatus.Downloading, progress.Status);
            Assert.AreEqual(100, progress.Percent.Value, 0.001);
            Assert.AreEqual("967.79KiB", progress.TotalSize);
            Assert.IsNull(progress.Speed);
            Assert.IsNull(progress.Eta);
        }

        [TestMethod]
        public void ParseLine_ForMerger_ReturnsConvertingStatusAndOutputPath()
        {
            var progress = YtDlpProgressParser.ParseLine(@"[Merger] Merging formats into ""D:\Videos\sample.mp4""");

            Assert.IsNotNull(progress);
            Assert.AreEqual(YtDlpJobStatus.Converting, progress.Status);
            Assert.AreEqual(@"D:\Videos\sample.mp4", progress.FilePath);
            Assert.AreEqual("Merging formats", progress.Message);
        }

        [TestMethod]
        public void ParseLine_ForUnknownLine_PreservesMessage()
        {
            var progress = YtDlpProgressParser.ParseLine("plain diagnostic line");

            Assert.IsNotNull(progress);
            Assert.AreEqual("plain diagnostic line", progress.Message);
            Assert.AreEqual("plain diagnostic line", progress.RawLine);
        }
    }
}
