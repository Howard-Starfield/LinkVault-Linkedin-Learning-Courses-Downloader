using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpDependencyStatusTests
    {
        [TestMethod]
        public void CanRun_ForVideoWithoutFfmpeg_ReturnsFalse()
        {
            var status = new YtDlpDependencyStatus
            {
                YtDlpPath = @"C:\Tools\yt-dlp.exe"
            };

            var canRun = status.CanRun(new YtDlpDownloadOptions
            {
                FormatChoice = YtDlpFormatChoice.Video
            });

            Assert.IsTrue(status.CanFetchMetadata);
            Assert.IsFalse(canRun);
            Assert.IsFalse(status.CanDownloadVideo);
        }

        [TestMethod]
        public void CanFetchMetadata_WithoutYtDlp_ReturnsFalse()
        {
            var status = new YtDlpDependencyStatus
            {
                FfmpegPath = @"C:\Tools\ffmpeg.exe"
            };

            Assert.IsFalse(status.CanFetchMetadata);
        }

        [TestMethod]
        public void CanRun_ForAudioWithFfmpeg_ReturnsTrue()
        {
            var status = new YtDlpDependencyStatus
            {
                YtDlpPath = @"C:\Tools\yt-dlp.exe",
                FfmpegPath = @"C:\Tools\ffmpeg.exe"
            };

            var canRun = status.CanRun(new YtDlpDownloadOptions
            {
                FormatChoice = YtDlpFormatChoice.Audio
            });

            Assert.IsTrue(canRun);
            Assert.IsTrue(status.CanMergeOrExtractAudio);
        }
    }
}
