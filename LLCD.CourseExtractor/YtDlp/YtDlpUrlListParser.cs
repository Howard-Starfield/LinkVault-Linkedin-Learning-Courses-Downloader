using System;
using System.Collections.Generic;
using System.Linq;

namespace LLCD.CourseExtractor.YtDlp
{
    public static class YtDlpUrlListParser
    {
        private static readonly char[] Separators = { '\r', '\n', '\t', ' ', ';' };

        public static List<string> Parse(string input)
        {
            if (String.IsNullOrWhiteSpace(input))
                return new List<string>();

            return input
                .Split(Separators, StringSplitOptions.RemoveEmptyEntries)
                .Select(url => url.Trim())
                .Where(url => !String.IsNullOrWhiteSpace(url))
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .ToList();
        }
    }
}
