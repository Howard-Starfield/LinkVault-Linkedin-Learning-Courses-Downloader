using System.Collections.Generic;
using System.Linq;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpCommand
    {
        public YtDlpCommand(string executablePath, IEnumerable<string> arguments)
        {
            ExecutablePath = executablePath;
            Arguments = arguments.ToList().AsReadOnly();
        }

        public string ExecutablePath { get; }

        public IReadOnlyList<string> Arguments { get; }
    }
}
