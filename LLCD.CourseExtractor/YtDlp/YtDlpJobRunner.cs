using System;
using System.IO;
using System.Threading;
using System.Threading.Tasks;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpJobRunner
    {
        private readonly YtDlpService _service;

        public YtDlpJobRunner(YtDlpService service)
        {
            _service = service ?? throw new ArgumentNullException(nameof(service));
        }

        public async Task<YtDlpDownloadResult> Download(YtDlpJob job, CancellationToken cancellationToken = default(CancellationToken), Action<YtDlpProgress> progressChanged = null)
        {
            if (job is null)
                throw new ArgumentNullException(nameof(job));
            if (job.Options is null)
                throw new ArgumentException("Job download options are required.", nameof(job));

            job.StartedAt = DateTimeOffset.UtcNow;
            job.Status = YtDlpJobStatus.Downloading;
            string outputFilePath = null;

            Action<string> handleLine = line =>
            {
                job.Logs.Add(line);
                var progress = YtDlpProgressParser.ParseLine(line);
                if (progress == null)
                    return;

                job.Progress = progress;
                if (!String.IsNullOrWhiteSpace(progress.FilePath))
                {
                    outputFilePath = progress.FilePath;
                }
                if (progress.Status != default(YtDlpJobStatus))
                {
                    job.Status = progress.Status;
                }
                progressChanged?.Invoke(progress);
            };

            try
            {
                var result = await _service.Download(job.Options, handleLine, handleLine, cancellationToken).ConfigureAwait(false);
                if (cancellationToken.IsCancellationRequested)
                {
                    job.CompletedAt = DateTimeOffset.UtcNow;
                    job.Status = YtDlpJobStatus.Cancelled;
                    job.ErrorMessage = "Download cancelled.";
                    throw new OperationCanceledException(cancellationToken);
                }

                job.CompletedAt = DateTimeOffset.UtcNow;
                job.Status = result.IsSuccess ? YtDlpJobStatus.Finished : YtDlpJobStatus.Failed;
                job.ErrorMessage = result.IsSuccess ? null : result.StandardError;
                job.OutputFilePath = outputFilePath;
                job.OutputFileName = GetFileName(outputFilePath);

                return new YtDlpDownloadResult
                {
                    Success = result.IsSuccess,
                    ExitCode = result.ExitCode,
                    FilePath = outputFilePath,
                    FileName = job.OutputFileName,
                    Error = result.IsSuccess ? null : result.StandardError,
                    StandardOutput = result.StandardOutput,
                    StandardError = result.StandardError
                };
            }
            catch (OperationCanceledException)
            {
                job.CompletedAt = DateTimeOffset.UtcNow;
                job.Status = YtDlpJobStatus.Cancelled;
                job.ErrorMessage = "Download cancelled.";
                throw;
            }
        }

        private static string GetFileName(string filePath)
        {
            if (String.IsNullOrWhiteSpace(filePath))
                return null;

            try
            {
                return Path.GetFileName(filePath);
            }
            catch (ArgumentException)
            {
                return null;
            }
        }
    }
}
