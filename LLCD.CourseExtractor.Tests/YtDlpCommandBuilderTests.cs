using System.Linq;
using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpCommandBuilderTests
    {
        [TestMethod]
        public void BuildMetadataCommand_WithBrowserCookies_ReturnsExpectedArguments()
        {
            var builder = new YtDlpCommandBuilder("custom-yt-dlp");

            var command = builder.BuildMetadataCommand("https://example.com/video", YtDlpBrowserCookiesSource.Chrome);

            Assert.AreEqual("custom-yt-dlp", command.ExecutablePath);
            CollectionAssert.AreEqual(new[]
            {
                "-J",
                "--no-playlist",
                "--cookies-from-browser",
                "chrome",
                "https://example.com/video"
            }, command.Arguments.ToArray());
        }

        [TestMethod]
        public void BuildDownloadCommand_ForVideoWithSubtitles_ReturnsExpectedArguments()
        {
            var builder = new YtDlpCommandBuilder();
            var options = new YtDlpDownloadOptions
            {
                Url = "https://example.com/video",
                OutputTemplate = @"D:\Downloads\%(title)s.%(ext)s",
                FfmpegLocation = @"C:\Tools\ffmpeg\bin",
                FormatChoice = YtDlpFormatChoice.Video,
                FormatId = "137",
                WriteSubtitles = true,
                WriteAutomaticSubtitles = true,
                SubtitleLanguages = "en,ja",
                CookiesSource = YtDlpBrowserCookiesSource.Edge,
                WriteInfoJson = true,
                WriteThumbnail = true
            };

            var command = builder.BuildDownloadCommand(options);

            CollectionAssert.AreEqual(new[]
            {
                "--newline",
                "--no-playlist",
                "-o",
                @"D:\Downloads\%(title)s.%(ext)s",
                "--cookies-from-browser",
                "edge",
                "--ffmpeg-location",
                @"C:\Tools\ffmpeg\bin",
                "--write-subs",
                "--write-auto-subs",
                "--sub-langs",
                "en,ja",
                "--convert-subs",
                "srt",
                "--write-info-json",
                "--write-thumbnail",
                "-f",
                "137+bestaudio/best",
                "--merge-output-format",
                "mp4",
                "https://example.com/video"
            }, command.Arguments.ToArray());
        }

        [TestMethod]
        public void BuildDownloadCommand_ForAudio_ReturnsAudioExtractionArguments()
        {
            var builder = new YtDlpCommandBuilder();
            var options = new YtDlpDownloadOptions
            {
                Url = "https://example.com/audio",
                OutputTemplate = "%(title)s.%(ext)s",
                FormatChoice = YtDlpFormatChoice.Audio,
                AudioFormat = YtDlpAudioFormat.Flac,
                WriteSubtitles = false,
                WriteAutomaticSubtitles = false
            };

            var command = builder.BuildDownloadCommand(options);

            CollectionAssert.AreEqual(new[]
            {
                "--newline",
                "--no-playlist",
                "-o",
                "%(title)s.%(ext)s",
                "-x",
                "--audio-format",
                "flac",
                "https://example.com/audio"
            }, command.Arguments.ToArray());
        }
    }
}
