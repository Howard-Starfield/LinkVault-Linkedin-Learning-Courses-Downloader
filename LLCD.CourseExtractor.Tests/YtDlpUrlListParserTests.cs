using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpUrlListParserTests
    {
        [TestMethod]
        public void Parse_WithLinesSpacesAndSemicolons_ReturnsDistinctUrls()
        {
            var urls = YtDlpUrlListParser.Parse(" https://example.com/one\r\nhttps://example.com/two; https://example.com/one ");

            Assert.AreEqual(2, urls.Count);
            Assert.AreEqual("https://example.com/one", urls[0]);
            Assert.AreEqual("https://example.com/two", urls[1]);
        }

        [TestMethod]
        public void Parse_WithEmptyInput_ReturnsEmptyList()
        {
            var urls = YtDlpUrlListParser.Parse(" \r\n\t ");

            Assert.AreEqual(0, urls.Count);
        }
    }
}
