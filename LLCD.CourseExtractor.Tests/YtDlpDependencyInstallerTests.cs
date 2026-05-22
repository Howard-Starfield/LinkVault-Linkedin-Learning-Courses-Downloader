using System;
using System.IO;
using System.IO.Compression;
using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpDependencyInstallerTests
    {
        [TestMethod]
        public void ExtractExecutableFromZip_ReplacesExistingExecutableAndDeletesTemp()
        {
            var root = CreateTempRoot();
            try
            {
                var zipPath = Path.Combine(root, "ffmpeg.zip");
                var targetDirectory = Path.Combine(root, "tools", "ffmpeg", "bin");
                var targetPath = Path.Combine(targetDirectory, "ffmpeg.exe");
                Directory.CreateDirectory(targetDirectory);
                File.WriteAllText(targetPath, "old");
                CreateZipWithEntry(zipPath, "ffmpeg-2026/bin/ffmpeg.exe", "new");

                var extractedPath = YtDlpDependencyInstaller.ExtractExecutableFromZip(zipPath, "ffmpeg.exe", targetDirectory);

                Assert.AreEqual(targetPath, extractedPath);
                Assert.AreEqual("new", File.ReadAllText(targetPath));
                Assert.IsFalse(File.Exists(targetPath + ".download"));
            }
            finally
            {
                DeleteTempRoot(root);
            }
        }

        [TestMethod]
        public void ExtractExecutableFromZip_WhenExecutableIsMissing_DoesNotCreateTemp()
        {
            var root = CreateTempRoot();
            try
            {
                var zipPath = Path.Combine(root, "ffmpeg.zip");
                var targetDirectory = Path.Combine(root, "tools", "ffmpeg", "bin");
                var targetPath = Path.Combine(targetDirectory, "ffmpeg.exe");
                Directory.CreateDirectory(targetDirectory);
                CreateZipWithEntry(zipPath, "ffmpeg-2026/bin/not-ffmpeg.exe", "content");

                try
                {
                    YtDlpDependencyInstaller.ExtractExecutableFromZip(zipPath, "ffmpeg.exe", targetDirectory);
                    Assert.Fail("Expected FileNotFoundException.");
                }
                catch (FileNotFoundException)
                {
                }

                Assert.IsFalse(File.Exists(targetPath));
                Assert.IsFalse(File.Exists(targetPath + ".download"));
            }
            finally
            {
                DeleteTempRoot(root);
            }
        }

        private static void CreateZipWithEntry(string zipPath, string entryName, string content)
        {
            using (var archive = ZipFile.Open(zipPath, ZipArchiveMode.Create))
            {
                var entry = archive.CreateEntry(entryName);
                using (var writer = new StreamWriter(entry.Open()))
                {
                    writer.Write(content);
                }
            }
        }

        private static string CreateTempRoot()
        {
            var root = Path.Combine(Path.GetTempPath(), "llcd-ytdlp-installer-test-" + Guid.NewGuid().ToString("N"));
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
