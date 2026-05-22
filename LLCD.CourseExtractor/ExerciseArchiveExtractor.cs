using System;
using System.IO;
using System.IO.Compression;
using System.Linq;

namespace LLCD.CourseExtractor
{
    public static class ExerciseArchiveExtractor
    {
        public static ExerciseArchiveExtractionResult ExtractZipAndDeleteArchive(string archivePath)
        {
            if (String.IsNullOrWhiteSpace(archivePath) || !File.Exists(archivePath))
            {
                return ExerciseArchiveExtractionResult.Failed(archivePath, null, "Archive file does not exist.");
            }

            if (!String.Equals(Path.GetExtension(archivePath), ".zip", StringComparison.OrdinalIgnoreCase))
            {
                return ExerciseArchiveExtractionResult.Skipped(archivePath);
            }

            string parentDirectory = Path.GetDirectoryName(Path.GetFullPath(archivePath));
            string archiveBaseName = Path.GetFileNameWithoutExtension(archivePath);
            string destinationDirectory = null;
            string temporaryDirectory = Path.Combine(
                parentDirectory,
                ".extracting-" + ToSafeFileName(archiveBaseName) + "-" + Guid.NewGuid().ToString("N"));

            try
            {
                Directory.CreateDirectory(temporaryDirectory);
                ExtractZipSafely(archivePath, temporaryDirectory);

                string extractedContentDirectory = GetExtractedContentDirectory(temporaryDirectory, archiveBaseName);
                string destinationName = extractedContentDirectory == temporaryDirectory
                    ? archiveBaseName
                    : Path.GetFileName(extractedContentDirectory);
                destinationDirectory = GetUniqueDirectoryPath(parentDirectory, destinationName);
                Directory.Move(extractedContentDirectory, destinationDirectory);
                if (!String.Equals(extractedContentDirectory, temporaryDirectory, StringComparison.OrdinalIgnoreCase))
                {
                    TryDeleteDirectory(temporaryDirectory);
                }

                string deleteWarning = null;
                try
                {
                    File.Delete(archivePath);
                }
                catch (Exception ex) when (ex is IOException || ex is UnauthorizedAccessException)
                {
                    deleteWarning = "Extracted successfully, but could not delete the zip file: " + ex.Message;
                }

                return ExerciseArchiveExtractionResult.Extracted(
                    archivePath,
                    destinationDirectory,
                    deleteWarning == null,
                    deleteWarning);
            }
            catch (Exception ex)
            {
                TryDeleteDirectory(temporaryDirectory);
                return ExerciseArchiveExtractionResult.Failed(archivePath, destinationDirectory, ex.Message);
            }
        }

        private static void ExtractZipSafely(string archivePath, string destinationDirectory)
        {
            string destinationRoot = Path.GetFullPath(destinationDirectory);
            if (!destinationRoot.EndsWith(Path.DirectorySeparatorChar.ToString(), StringComparison.Ordinal))
            {
                destinationRoot += Path.DirectorySeparatorChar;
            }

            using (var zipStream = File.OpenRead(archivePath))
            using (var archive = new ZipArchive(zipStream, ZipArchiveMode.Read))
            {
                foreach (var entry in archive.Entries.Where(entry => !String.IsNullOrWhiteSpace(entry.FullName)))
                {
                    string entryPath = entry.FullName.Replace('/', Path.DirectorySeparatorChar);
                    string targetPath = Path.GetFullPath(Path.Combine(destinationDirectory, entryPath));
                    if (!targetPath.StartsWith(destinationRoot, StringComparison.OrdinalIgnoreCase))
                    {
                        throw new InvalidDataException("Archive contains an unsafe file path: " + entry.FullName);
                    }

                    if (String.IsNullOrEmpty(entry.Name))
                    {
                        Directory.CreateDirectory(targetPath);
                        continue;
                    }

                    string entryDirectory = Path.GetDirectoryName(targetPath);
                    if (!String.IsNullOrEmpty(entryDirectory))
                    {
                        Directory.CreateDirectory(entryDirectory);
                    }

                    using (var entryStream = entry.Open())
                    using (var outputStream = File.Create(targetPath))
                    {
                        entryStream.CopyTo(outputStream);
                    }
                }
            }
        }

        private static string GetUniqueDirectoryPath(string parentDirectory, string requestedName)
        {
            string safeName = ToSafeFileName(requestedName);
            if (String.IsNullOrWhiteSpace(safeName))
            {
                safeName = "Exercise Files";
            }

            string candidate = Path.Combine(parentDirectory, safeName);
            int suffix = 2;
            while (Directory.Exists(candidate) || File.Exists(candidate))
            {
                candidate = Path.Combine(parentDirectory, $"{safeName} ({suffix})");
                suffix++;
            }
            return candidate;
        }

        private static string GetExtractedContentDirectory(string temporaryDirectory, string archiveBaseName)
        {
            string[] topLevelFiles = Directory.GetFiles(temporaryDirectory);
            string[] topLevelDirectories = Directory.GetDirectories(temporaryDirectory);
            if (topLevelFiles.Length == 0 &&
                topLevelDirectories.Length == 1 &&
                String.Equals(Path.GetFileName(topLevelDirectories[0]), archiveBaseName, StringComparison.OrdinalIgnoreCase))
            {
                return topLevelDirectories[0];
            }

            return temporaryDirectory;
        }

        private static string ToSafeFileName(string fileName)
        {
            return string.Concat(fileName.Split(Path.GetInvalidFileNameChars()));
        }

        private static void TryDeleteDirectory(string directoryPath)
        {
            try
            {
                if (!String.IsNullOrWhiteSpace(directoryPath) && Directory.Exists(directoryPath))
                {
                    Directory.Delete(directoryPath, true);
                }
            }
            catch
            {
            }
        }
    }

    public class ExerciseArchiveExtractionResult
    {
        private ExerciseArchiveExtractionResult(
            string archivePath,
            string destinationDirectory,
            bool attempted,
            bool succeeded,
            bool archiveDeleted,
            string message)
        {
            ArchivePath = archivePath;
            DestinationDirectory = destinationDirectory;
            Attempted = attempted;
            Succeeded = succeeded;
            ArchiveDeleted = archiveDeleted;
            Message = message;
        }

        public string ArchivePath { get; }
        public string DestinationDirectory { get; }
        public bool Attempted { get; }
        public bool Succeeded { get; }
        public bool ArchiveDeleted { get; }
        public string Message { get; }

        public static ExerciseArchiveExtractionResult Skipped(string archivePath)
        {
            return new ExerciseArchiveExtractionResult(archivePath, null, false, false, false, null);
        }

        public static ExerciseArchiveExtractionResult Extracted(
            string archivePath,
            string destinationDirectory,
            bool archiveDeleted,
            string message)
        {
            return new ExerciseArchiveExtractionResult(archivePath, destinationDirectory, true, true, archiveDeleted, message);
        }

        public static ExerciseArchiveExtractionResult Failed(string archivePath, string destinationDirectory, string message)
        {
            return new ExerciseArchiveExtractionResult(archivePath, destinationDirectory, true, false, false, message);
        }
    }
}
