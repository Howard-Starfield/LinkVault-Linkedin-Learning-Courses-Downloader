using System;
using System.Threading;
using System.Threading.Tasks;
using LLCD.CourseExtractor;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class RetryTests
    {
        [TestMethod]
        public async Task DoAsync_WhenOperationIsCanceled_PropagatesCancellationWithoutRetry()
        {
            int attempts = 0;
            int errorCallbacks = 0;
            using (var cancellationTokenSource = new CancellationTokenSource())
            {
                cancellationTokenSource.Cancel();

                await ExpectCanceled(async () =>
                {
                    await Retry.Do(() =>
                    {
                        attempts++;
                        return Task.FromCanceled(cancellationTokenSource.Token);
                    }, "cancelled", () => errorCallbacks++, retries: 3);
                });
            }

            Assert.AreEqual(1, attempts);
            Assert.AreEqual(0, errorCallbacks);
        }

        [TestMethod]
        public async Task DoAsync_WhenOperationEventuallySucceeds_RetriesFailures()
        {
            int attempts = 0;
            int errorCallbacks = 0;

            await Retry.Do(async () =>
            {
                attempts++;
                if (attempts == 1)
                {
                    throw new InvalidOperationException("transient");
                }
                await Task.CompletedTask;
            }, "transient failure", () => errorCallbacks++, retries: 3);

            Assert.AreEqual(2, attempts);
            Assert.AreEqual(1, errorCallbacks);
        }

        [TestMethod]
        public async Task DoAsyncWithResult_WhenOperationIsCanceled_PropagatesCancellationWithoutRetry()
        {
            int attempts = 0;
            using (var cancellationTokenSource = new CancellationTokenSource())
            {
                cancellationTokenSource.Cancel();

                await ExpectCanceled(async () =>
                {
                    await Retry.Do<int>(() =>
                    {
                        attempts++;
                        return Task.FromCanceled<int>(cancellationTokenSource.Token);
                    }, "cancelled", retries: 3);
                });
            }

            Assert.AreEqual(1, attempts);
        }

        [TestMethod]
        public async Task DoAsync_WhenFailuresExhaustRetries_RunsFatalAction()
        {
            int attempts = 0;
            int errorCallbacks = 0;
            int fatalCallbacks = 0;

            await Retry.Do(() =>
            {
                attempts++;
                return Task.FromException(new InvalidOperationException("persistent"));
            }, "persistent failure", () => errorCallbacks++, () => fatalCallbacks++, retries: 2);

            Assert.AreEqual(2, attempts);
            Assert.AreEqual(2, errorCallbacks);
            Assert.AreEqual(1, fatalCallbacks);
        }

        private static async Task ExpectCanceled(Func<Task> action)
        {
            try
            {
                await action();
            }
            catch (OperationCanceledException)
            {
                return;
            }

            Assert.Fail("Expected OperationCanceledException.");
        }
    }
}
