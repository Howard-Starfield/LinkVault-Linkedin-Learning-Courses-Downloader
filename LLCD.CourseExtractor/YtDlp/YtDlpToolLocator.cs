using System;
using System.IO;
using System.Linq;

namespace LLCD.CourseExtractor.YtDlp
{
    public static class YtDlpToolLocator
    {
        public static string FindYtDlp(string appRootDirectory = null)
        {
            var appLocal = FindAppLocalExecutable(appRootDirectory, Path.Combine("tools", "yt-dlp", "yt-dlp.exe"))
                ?? FindAppLocalExecutable(appRootDirectory, "yt-dlp.exe");
            if (!String.IsNullOrWhiteSpace(appLocal))
                return appLocal;

            return FindExecutable("yt-dlp.exe") ?? FindExecutable("yt-dlp");
        }

        public static string FindFfmpeg(string appRootDirectory = null)
        {
            var appLocal = FindAppLocalExecutable(appRootDirectory, Path.Combine("tools", "ffmpeg", "bin", "ffmpeg.exe"))
                ?? FindAppLocalExecutable(appRootDirectory, "ffmpeg.exe");
            if (!String.IsNullOrWhiteSpace(appLocal))
                return appLocal;

            return FindExecutable("ffmpeg.exe") ?? FindExecutable("ffmpeg");
        }

        public static string FindFfprobe(string appRootDirectory = null)
        {
            var appLocal = FindAppLocalExecutable(appRootDirectory, Path.Combine("tools", "ffmpeg", "bin", "ffprobe.exe"))
                ?? FindAppLocalExecutable(appRootDirectory, "ffprobe.exe");
            if (!String.IsNullOrWhiteSpace(appLocal))
                return appLocal;

            return FindExecutable("ffprobe.exe") ?? FindExecutable("ffprobe");
        }

        private static string FindAppLocalExecutable(string appRootDirectory, string relativePath)
        {
            var roots = new[]
            {
                appRootDirectory,
                AppDomain.CurrentDomain.BaseDirectory,
                Directory.GetCurrentDirectory()
            };

            foreach (var root in roots)
            {
                if (String.IsNullOrWhiteSpace(root))
                    continue;

                string candidate;
                try
                {
                    candidate = Path.Combine(root, relativePath);
                }
                catch (ArgumentException)
                {
                    continue;
                }

                if (File.Exists(candidate))
                    return Path.GetFullPath(candidate);
            }

            return null;
        }

        private static string FindExecutable(string executableName)
        {
            if (String.IsNullOrWhiteSpace(executableName))
                return null;

            if (File.Exists(executableName))
                return Path.GetFullPath(executableName);

            string path = Environment.GetEnvironmentVariable("PATH");
            if (String.IsNullOrWhiteSpace(path))
                return null;

            return path
                .Split(Path.PathSeparator)
                .Where(dir => !String.IsNullOrWhiteSpace(dir))
                .Select(dir => SafeCombine(dir, executableName))
                .FirstOrDefault(File.Exists);
        }

        private static string SafeCombine(string directory, string executableName)
        {
            try
            {
                return Path.Combine(directory.Trim(), executableName);
            }
            catch (ArgumentException)
            {
                return String.Empty;
            }
        }
    }
}
