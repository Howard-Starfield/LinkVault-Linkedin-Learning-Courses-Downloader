using System.Linq;
using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpInfoTests
    {
        [TestMethod]
        public void FromJson_ParsesMetadataFormatsAndSubtitles()
        {
            const string json = @"{
  ""title"": ""Sample Video"",
  ""thumbnail"": ""https://example.com/thumb.jpg"",
  ""duration"": 123.5,
  ""uploader"": ""Example Channel"",
  ""webpage_url"": ""https://example.com/watch"",
  ""formats"": [
    { ""format_id"": ""18"", ""height"": 360, ""tbr"": 800, ""vcodec"": ""avc1"", ""acodec"": ""mp4a"" },
    { ""format_id"": ""22"", ""height"": 720, ""tbr"": 1500, ""vcodec"": ""avc1"", ""acodec"": ""mp4a"" },
    { ""format_id"": ""136"", ""height"": 720, ""tbr"": 1100, ""vcodec"": ""avc1"", ""acodec"": ""none"" },
    { ""format_id"": ""140"", ""tbr"": 128, ""vcodec"": ""none"", ""acodec"": ""mp4a"" }
  ],
  ""subtitles"": {
    ""en"": [{ ""ext"": ""vtt"", ""name"": ""English"" }],
    ""ja"": [{ ""ext"": ""vtt"", ""name"": ""Japanese"" }]
  },
  ""automatic_captions"": {
    ""en"": [{ ""ext"": ""srv3"", ""name"": ""English auto"" }]
  }
}";

            var info = YtDlpInfo.FromJson(json);

            Assert.AreEqual("Sample Video", info.Title);
            Assert.AreEqual("https://example.com/thumb.jpg", info.Thumbnail);
            Assert.AreEqual(123.5, info.Duration);
            Assert.AreEqual("Example Channel", info.Uploader);
            Assert.AreEqual("https://example.com/watch", info.Url);

            Assert.AreEqual(2, info.Formats.Count);
            Assert.AreEqual("22", info.Formats[0].Id);
            Assert.AreEqual("720p", info.Formats[0].Label);
            Assert.AreEqual("18", info.Formats[1].Id);

            Assert.AreEqual(2, info.Subtitles.Count);
            Assert.IsTrue(info.Subtitles.Any(track => track.Language == "en" && track.Extension == "vtt" && !track.IsAutomatic));
            Assert.IsTrue(info.Subtitles.Any(track => track.Language == "ja" && track.Name == "Japanese"));

            Assert.AreEqual(1, info.AutomaticCaptions.Count);
            Assert.AreEqual("en", info.AutomaticCaptions[0].Language);
            Assert.IsTrue(info.AutomaticCaptions[0].IsAutomatic);
        }
    }
}
