using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpPlaylistInfoTests
    {
        [TestMethod]
        public void FromJson_WithPlaylistEntries_ReturnsQueueEntries()
        {
            const string json = @"{
  ""_type"": ""playlist"",
  ""title"": ""Sample Playlist"",
  ""webpage_url"": ""https://example.com/playlist"",
  ""entries"": [
    { ""id"": ""a"", ""title"": ""First"", ""webpage_url"": ""https://example.com/watch/a"", ""playlist_index"": 1 },
    { ""id"": ""b"", ""title"": ""Second"", ""url"": ""https://example.com/watch/b"", ""playlist_index"": 2 }
  ]
}";

            var info = YtDlpPlaylistInfo.FromJson(json);

            Assert.IsTrue(info.IsPlaylist);
            Assert.AreEqual("Sample Playlist", info.Title);
            Assert.AreEqual(2, info.Entries.Count);
            Assert.AreEqual("First", info.Entries[0].Title);
            Assert.AreEqual("https://example.com/watch/a", info.Entries[0].Url);
            Assert.AreEqual(2, info.Entries[1].Index);
        }

        [TestMethod]
        public void FromJson_WithSingleVideo_ReturnsSingleQueueEntry()
        {
            const string json = @"{
  ""title"": ""Single Video"",
  ""webpage_url"": ""https://example.com/watch/single"",
  ""id"": ""single""
}";

            var info = YtDlpPlaylistInfo.FromJson(json);

            Assert.IsTrue(info.IsPlaylist);
            Assert.AreEqual(1, info.Entries.Count);
            Assert.AreEqual("Single Video", info.Entries[0].Title);
            Assert.AreEqual("https://example.com/watch/single", info.Entries[0].Url);
        }
    }
}
