using System;
using System.IO;
using System.IO.Compression;
using LLCD.CourseExtractor;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class ExerciseArchiveExtractorTests
    {
        [TestMethod]
        public void ExtractZipAndDeleteArchive_WithValidZip_ExtractsToFolderAndDeletesZip()
        {
            string root = CreateTempDirectory();
            try
            {
                string zipPath = Path.Combine(root, "exercise.zip");
                CreateZip(zipPath, archive =>
                {
                    var entry = archive.CreateEntry("chapter-1/readme.txt");
                    using (var writer = new StreamWriter(entry.Open()))
                    {
                        writer.Write("hello");
                    }
                });

                var result = ExerciseArchiveExtractor.ExtractZipAndDeleteArchive(zipPath);

                Assert.IsTrue(result.Attempted);
                Assert.IsTrue(result.Succeeded);
                Assert.IsTrue(result.ArchiveDeleted);
                Assert.IsFalse(File.Exists(zipPath));
                Assert.AreEqual("hello", File.ReadAllText(Path.Combine(root, "exercise", "chapter-1", "readme.txt")));
            }
            finally
            {
                Directory.Delete(root, true);
            }
        }

        [TestMethod]
        public void ExtractZipAndDeleteArchive_WithSingleRootFolder_DoesNotDuplicateRootFolder()
        {
            string root = CreateTempDirectory();
            try
            {
                string zipPath = Path.Combine(root, "Ex_Files_Sample.zip");
                CreateZip(zipPath, archive =>
                {
                    var entry = archive.CreateEntry("Ex_Files_Sample/start.txt");
                    using (var writer = new StreamWriter(entry.Open()))
                    {
                        writer.Write("ready");
                    }
                });

                var result = ExerciseArchiveExtractor.ExtractZipAndDeleteArchive(zipPath);

                Assert.IsTrue(result.Succeeded);
                Assert.IsFalse(File.Exists(zipPath));
                Assert.IsTrue(File.Exists(Path.Combine(root, "Ex_Files_Sample", "start.txt")));
                Assert.IsFalse(Directory.Exists(Path.Combine(root, "Ex_Files_Sample", "Ex_Files_Sample")));
            }
            finally
            {
                Directory.Delete(root, true);
            }
        }

        [TestMethod]
        public void ExtractZipAndDeleteArchive_WithNonZipFile_SkipsAndKeepsFile()
        {
            string root = CreateTempDirectory();
            try
            {
                string filePath = Path.Combine(root, "notes.txt");
                File.WriteAllText(filePath, "not an archive");

                var result = ExerciseArchiveExtractor.ExtractZipAndDeleteArchive(filePath);

                Assert.IsFalse(result.Attempted);
                Assert.IsFalse(result.Succeeded);
                Assert.IsTrue(File.Exists(filePath));
            }
            finally
            {
                Directory.Delete(root, true);
            }
        }

        [TestMethod]
        public void ExtractZipAndDeleteArchive_WithUnsafeZipPath_FailsAndKeepsZip()
        {
            string root = CreateTempDirectory();
            try
            {
                string zipPath = Path.Combine(root, "unsafe.zip");
                CreateZip(zipPath, archive =>
                {
                    var entry = archive.CreateEntry("../outside.txt");
                    using (var writer = new StreamWriter(entry.Open()))
                    {
                        writer.Write("escape");
                    }
                });

                var result = ExerciseArchiveExtractor.ExtractZipAndDeleteArchive(zipPath);

                Assert.IsTrue(result.Attempted);
                Assert.IsFalse(result.Succeeded);
                Assert.IsTrue(File.Exists(zipPath));
                Assert.IsFalse(File.Exists(Path.Combine(root, "..", "outside.txt")));
                Assert.AreEqual(0, Directory.GetDirectories(root, ".extracting-*").Length);
            }
            finally
            {
                Directory.Delete(root, true);
            }
        }

        private static string CreateTempDirectory()
        {
            string root = Path.Combine(Path.GetTempPath(), "llcd-archive-test-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(root);
            return root;
        }

        private static void CreateZip(string zipPath, Action<ZipArchive> configure)
        {
            using (var archive = ZipFile.Open(zipPath, ZipArchiveMode.Create))
            {
                configure(archive);
            }
        }
    }
}
