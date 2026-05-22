using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpProcessRunner : IYtDlpProcessRunner
    {
        public Task<YtDlpProcessResult> RunAsync(YtDlpCommand command, CancellationToken cancellationToken, Action<string> outputLine = null, Action<string> errorLine = null)
        {
            if (command is null)
                throw new ArgumentNullException(nameof(command));
            if (cancellationToken.IsCancellationRequested)
                return Task.FromCanceled<YtDlpProcessResult>(cancellationToken);

            var completion = new TaskCompletionSource<YtDlpProcessResult>(TaskCreationOptions.RunContinuationsAsynchronously);
            var stdout = new StringBuilder();
            var stderr = new StringBuilder();
            CancellationTokenRegistration cancellationRegistration = default(CancellationTokenRegistration);

            var process = new Process
            {
                StartInfo = new ProcessStartInfo
                {
                    FileName = command.ExecutablePath,
                    Arguments = ToArgumentString(command),
                    UseShellExecute = false,
                    CreateNoWindow = true,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true
                },
                EnableRaisingEvents = true
            };

            process.OutputDataReceived += (sender, args) =>
            {
                if (args.Data == null)
                    return;

                stdout.AppendLine(args.Data);
                outputLine?.Invoke(args.Data);
            };
            process.ErrorDataReceived += (sender, args) =>
            {
                if (args.Data == null)
                    return;

                stderr.AppendLine(args.Data);
                errorLine?.Invoke(args.Data);
            };
            process.Exited += (sender, args) =>
            {
                try
                {
                    process.WaitForExit();
                    completion.TrySetResult(new YtDlpProcessResult
                    {
                        ExitCode = process.ExitCode,
                        StandardOutput = stdout.ToString(),
                        StandardError = stderr.ToString()
                    });
                }
                catch (Exception ex)
                {
                    completion.TrySetException(ex);
                }
                finally
                {
                    process.Dispose();
                }
            };

            try
            {
                process.Start();
                process.BeginOutputReadLine();
                process.BeginErrorReadLine();
            }
            catch (Exception ex)
            {
                process.Dispose();
                completion.TrySetException(ex);
            }

            if (cancellationToken.CanBeCanceled)
            {
                cancellationRegistration = cancellationToken.Register(() =>
                {
                    try
                    {
                        if (!process.HasExited)
                            KillProcessTree(process);
                    }
                    catch (Exception ex) when (ex is InvalidOperationException || ex is ObjectDisposedException)
                    {
                    }
                });
            }

            completion.Task.ContinueWith(task => cancellationRegistration.Dispose(), TaskScheduler.Default);
            return completion.Task;
        }

        internal static void KillProcessTree(Process process)
        {
            if (process is null || process.HasExited)
                return;

            if (IsWindows())
            {
                try
                {
                    using (var taskKill = Process.Start(new ProcessStartInfo
                    {
                        FileName = "taskkill.exe",
                        Arguments = "/PID " + process.Id + " /T /F",
                        CreateNoWindow = true,
                        UseShellExecute = false,
                        RedirectStandardOutput = true,
                        RedirectStandardError = true
                    }))
                    {
                        taskKill?.WaitForExit(5000);
                    }
                    return;
                }
                catch (Exception ex) when (ex is InvalidOperationException || ex is Win32Exception || ex is IOException)
                {
                    if (process.HasExited)
                        return;
                }
            }

            process.Kill();
        }

        internal static bool IsWindows()
        {
            return Path.DirectorySeparatorChar == '\\';
        }

        internal static string ToArgumentString(YtDlpCommand command)
        {
            return String.Join(" ", System.Linq.Enumerable.Select(command.Arguments, QuoteArgument));
        }

        private static string QuoteArgument(string argument)
        {
            if (argument is null)
                return "\"\"";
            if (argument.Length == 0)
                return "\"\"";
            if (argument.IndexOfAny(new[] { ' ', '\t', '\n', '\r', '"' }) < 0)
                return argument;

            var quoted = new StringBuilder();
            quoted.Append('"');
            int backslashCount = 0;
            foreach (char c in argument)
            {
                if (c == '\\')
                {
                    backslashCount++;
                    continue;
                }

                if (c == '"')
                {
                    quoted.Append('\\', backslashCount * 2 + 1);
                    quoted.Append('"');
                    backslashCount = 0;
                    continue;
                }

                if (backslashCount > 0)
                {
                    quoted.Append('\\', backslashCount);
                    backslashCount = 0;
                }
                quoted.Append(c);
            }
            if (backslashCount > 0)
            {
                quoted.Append('\\', backslashCount * 2);
            }
            quoted.Append('"');
            return quoted.ToString();
        }
    }
}
