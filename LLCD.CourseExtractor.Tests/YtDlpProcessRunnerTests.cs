using System.Linq;
using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpProcessRunnerTests
    {
        [TestMethod]
        public void ToArgumentString_ForPathWithSpaces_DoesNotDoubleNormalBackslashes()
        {
            var command = new YtDlpCommand("yt-dlp", new[]
            {
                "--ffmpeg-location",
                @"C:\Program Files\FFmpeg\bin\ffmpeg.exe"
            });

            var arguments = YtDlpProcessRunner.ToArgumentString(command);

            Assert.AreEqual(@"--ffmpeg-location ""C:\Program Files\FFmpeg\bin\ffmpeg.exe""", arguments);
        }

        [TestMethod]
        public void ToArgumentString_ForQuotedArgument_EscapesEmbeddedQuotes()
        {
            var command = new YtDlpCommand("yt-dlp", new[]
            {
                @"D:\Videos\course ""intro""\%(title)s.%(ext)s"
            });

            var arguments = YtDlpProcessRunner.ToArgumentString(command);

            Assert.AreEqual(@"""D:\Videos\course \""intro\""\%(title)s.%(ext)s""", arguments);
        }

        [TestMethod]
        public void ToArgumentString_ForTrailingBackslashBeforeClosingQuote_PreservesBackslash()
        {
            var command = new YtDlpCommand("yt-dlp", Enumerable.Repeat(@"C:\Path With Space\", 1));

            var arguments = YtDlpProcessRunner.ToArgumentString(command);

            Assert.AreEqual(@"""C:\Path With Space\\""", arguments);
        }

        [TestMethod]
        public void IsWindows_UsesDirectorySeparator()
        {
            Assert.AreEqual(System.IO.Path.DirectorySeparatorChar == '\\', YtDlpProcessRunner.IsWindows());
        }
    }
}
