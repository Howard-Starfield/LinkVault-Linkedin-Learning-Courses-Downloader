using Serilog;
using System;
using System.Threading.Tasks;

namespace LLCD.CourseExtractor
{
    public class Retry
    {
        public static T Do<T>(Func<T> function, string exceptionMessage, Action actionOnError = null, Action actionOnFatal = null, int retries = 5)
        {
            int totalRetries = retries;
            do
            {
                try
                {
                    return function();
                }
                catch (OperationCanceledException)
                {
                    throw;
                }
                catch (Exception ex)
                {
                    Log.Error(ex, exceptionMessage);
                    if (!(actionOnError is null))
                    {
                        Log.Information("Running Action on Error");
                        actionOnError();
                    }
                    Log.Error("Trying again ,{retryCount}", (totalRetries + 1) - retries--);
                }

            }
            while (retries > 0);


            Log.Fatal("Error occured {retries} times without being resolved", totalRetries);
            if (actionOnFatal is null)
            {
                throw new Exception("A fatal error occured in the program. Check the logs for more info");
            }
            else
            {
                Log.Information("Running action on fatal");
                actionOnFatal();
                return default;
            }
        }

        public static async Task<T> Do<T>(Func<Task<T>> function, string exceptionMessage, Action actionOnError = null, Action actionOnFatal = null, int retries = 5)
        {
            int totalRetries = retries;
            do
            {
                try
                {
                    return await function().ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    throw;
                }
                catch (Exception ex)
                {
                    Log.Error(ex, exceptionMessage);
                    if (!(actionOnError is null))
                    {
                        Log.Information("Running Action on Error");
                        actionOnError();
                    }
                    Log.Error("Trying again {retryCount}", (totalRetries + 1) - retries--);
                }

            }
            while (retries > 0);


            Log.Fatal("Error occured {retries} times without being resolved", totalRetries);
            if (actionOnFatal is null)
            {
                throw new Exception("A fatal error occured in the program. Check the logs for more info");
            }
            else
            {
                Log.Information("Running action on fatal");
                actionOnFatal();
                return default;
            }
        }

        public static void Do(Action function, string exceptionMessage, Action actionOnError = null, Action actionOnFatal = null, int retries = 5)
        {
            Do(new Func<object>(() =>
            {
                function();
                return null;
            }), exceptionMessage, actionOnError, actionOnFatal, retries);
        }

        public static async Task Do(Func<Task> function, string exceptionMessage, Action actionOnError = null, Action actionOnFatal = null, int retries = 5)
        {
            int totalRetries = retries;
            do
            {
                try
                {
                    await function().ConfigureAwait(false);
                    return;
                }
                catch (OperationCanceledException)
                {
                    throw;
                }
                catch (Exception ex)
                {
                    Log.Error(ex, exceptionMessage);
                    if (!(actionOnError is null))
                    {
                        Log.Information("Running Action on Error");
                        actionOnError();
                    }
                    Log.Error("Trying again {retryCount}", (totalRetries + 1) - retries--);
                }

            }
            while (retries > 0);


            Log.Fatal("Error occured {retries} times without being resolved", totalRetries);
            if (actionOnFatal is null)
            {
                throw new Exception("A fatal error occured in the program. Check the logs for more info");
            }
            else
            {
                Log.Information("Running action on fatal");
                actionOnFatal();
            }
        }

    }
}
