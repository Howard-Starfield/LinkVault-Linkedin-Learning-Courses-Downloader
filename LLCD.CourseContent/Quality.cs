using System;

namespace LLCD.CourseContent
{
    public enum Quality
    {
        BestAvailable,
        High,
        Medium,
        Low
    }

    public static class QualityExtensions
    {
        public static string ToHeight(this Quality q)
        {
            return q.ToHeights()[0];
        }

        public static string[] ToHeights(this Quality q)
        {
            switch (q)
            {
                case Quality.BestAvailable:
                    return new[] { "1080", "720", "540", "360" };
                case Quality.High:
                    return new[] { "720", "540", "360" };
                case Quality.Medium:
                    return new[] { "540", "360" };
                case Quality.Low:
                    return new[] { "360" };
                default:
                    throw new ArgumentException("Undefined Quality");
            }
        }
    }
}
