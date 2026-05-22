using System;
using System.IO;
using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpToolLocatorTests
    {
        [TestMethod]
        public void FindYtDlp_WithAppLocalTool_ReturnsAppLocalPath()
        {
            var root = CreateTempRoot();
            try
            {
                var toolPath = Path.Combine(root, "tools", "yt-dlp", "yt-dlp.exe");
                Directory.CreateDirectory(Path.GetDirectoryName(toolPath));
                File.WriteAllText(toolPath, String.Empty);

                var detected = YtDlpToolLocator.FindYtDlp(root);

                Assert.AreEqual(Path.GetFullPath(toolPath), detected);
            }
            finally
            {
                DeleteTempRoot(root);
            }
        }

        [TestMethod]
        public void FindFfmpeg_WithAppLocalTool_ReturnsAppLocalPath()
        {
            var root = CreateTempRoot();
            try
            {
                var toolPath = Path.Combine(root, "tools", "ffmpeg", "bin", "ffmpeg.exe");
                Directory.CreateDirectory(Path.GetDirectoryName(toolPath));
                File.WriteAllText(toolPath, String.Empty);

                var detected = YtDlpToolLocator.FindFfmpeg(root);

                Assert.AreEqual(Path.GetFullPath(toolPath), detected);
            }
            finally
            {
                DeleteTempRoot(root);
            }
        }

        [TestMethod]
        public void Check_WithAppLocalTools_ReturnsAllPaths()
        {
            var root = CreateTempRoot();
            try
            {
                var ytDlpPath = Path.Combine(root, "tools", "yt-dlp", "yt-dlp.exe");
                var ffmpegPath = Path.Combine(root, "tools", "ffmpeg", "bin", "ffmpeg.exe");
                var ffprobePath = Path.Combine(root, "tools", "ffmpeg", "bin", "ffprobe.exe");
                Directory.CreateDirectory(Path.GetDirectoryName(ytDlpPath));
                Directory.CreateDirectory(Path.GetDirectoryName(ffmpegPath));
                File.WriteAllText(ytDlpPath, String.Empty);
                File.WriteAllText(ffmpegPath, String.Empty);
                File.WriteAllText(ffprobePath, String.Empty);

                var status = YtDlpDependencyChecker.Check(root);

                Assert.AreEqual(Path.GetFullPath(ytDlpPath), status.YtDlpPath);
                Assert.AreEqual(Path.GetFullPath(ffmpegPath), status.FfmpegPath);
                Assert.AreEqual(Path.GetFullPath(ffprobePath), status.FfprobePath);
                Assert.IsTrue(status.HasYtDlp);
                Assert.IsTrue(status.HasFfmpeg);
                Assert.IsTrue(status.HasFfprobe);
            }
            finally
            {
                DeleteTempRoot(root);
            }
        }

        private static string CreateTempRoot()
        {
            var root = Path.Combine(Path.GetTempPath(), "llcd-ytdlp-test-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(root);
            return root;
        }

        private static void DeleteTempRoot(string root)
        {
            if (String.IsNullOrWhiteSpace(root) || !Directory.Exists(root))
                return;

            Directory.Delete(root, true);
        }
    }
}
