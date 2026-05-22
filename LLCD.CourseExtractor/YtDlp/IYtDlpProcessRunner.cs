using System;
using System.Threading;
using System.Threading.Tasks;

namespace LLCD.CourseExtractor.YtDlp
{
    public interface IYtDlpProcessRunner
    {
        Task<YtDlpProcessResult> RunAsync(YtDlpCommand command, CancellationToken cancellationToken, Action<string> outputLine = null, Action<string> errorLine = null);
    }
}
