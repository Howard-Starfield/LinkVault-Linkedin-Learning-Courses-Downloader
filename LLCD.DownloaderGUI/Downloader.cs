using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace LLCD.DownloaderGUI
{
    class Downloader : IDisposable
    {
        private HttpClient _httpClient = new HttpClient
        {
            Timeout = TimeSpan.FromMinutes(30)
        };
        public void Dispose()
        {
            _httpClient.Dispose();
        }

        //https://github.com/dotnet/runtime/issues/31479#issuecomment-578436466
        /// <summary>
        /// Downloads a file from the specified Uri into the specified stream.
        /// </summary>
        /// <param name="cancellationToken">An optional CancellationToken that can be used to cancel the in-progress download.</param>
        /// <param name="progressCallback">If not null, will be called as the download progress. The first parameter will be the number of bytes downloaded so far, and the second the total size of the expected file after download.</param>
        /// <returns>A task that is completed once the download is complete.</returns>
        public async Task DownloadFileAsync(Uri uri, Stream toStream, CancellationToken cancellationToken = default, Action<long, long> progressCallback = null)
        {
            if (uri == null)
                throw new ArgumentNullException(nameof(uri));
            if (toStream == null)
                throw new ArgumentNullException(nameof(toStream));

            if (uri.IsFile)
            {
                using (Stream file = File.OpenRead(uri.LocalPath))
                {
                    if (progressCallback != null)
                    {
                        long length = file.Length;
                        byte[] buffer = new byte[4096];
                        int read;
                        long totalRead = 0;
                        while ((read = await file.ReadAsync(buffer, 0, buffer.Length, cancellationToken).ConfigureAwait(false)) > 0)
                        {
                            await toStream.WriteAsync(buffer, 0, read, cancellationToken).ConfigureAwait(false);
                            totalRead += read;
                            progressCallback(totalRead, length);
                        }
                        Debug.Assert(totalRead == length || length == -1);
                    }
                    else
                    {
                        await file.CopyToAsync(toStream, 4096, cancellationToken).ConfigureAwait(false);
                    }
                }


            }
            else
            {
                using (HttpResponseMessage response = await _httpClient.GetAsync(uri, HttpCompletionOption.ResponseHeadersRead, cancellationToken).ConfigureAwait(false))
                {
                    response.EnsureSuccessStatusCode();
                    if (progressCallback != null)
                    {
                        long length = response.Content.Headers.ContentLength ?? -1;
                        using (Stream stream = await response.Content.ReadAsStreamAsync().ConfigureAwait(false))
                        {
                            byte[] buffer = new byte[16384];
                            int read;
                            long totalRead = 0;
                            while ((read = await stream.ReadAsync(buffer, 0, buffer.Length, cancellationToken).ConfigureAwait(false)) > 0)
                            {
                                await toStream.WriteAsync(buffer, 0, read, cancellationToken).ConfigureAwait(false);
                                totalRead += read;
                                progressCallback(totalRead, length);
                            }
                            if (length != -1 && totalRead != length)
                            {
                                throw new IOException($"Incomplete download. Expected {length} bytes but received {totalRead} bytes.");
                            }
                        }
                    }
                    else
                    {
                        await response.Content.CopyToAsync(toStream).ConfigureAwait(false);
                    }
                }
            }
        }

        public async Task DownloadFileAsync(Uri uri, string filePath, CancellationToken cancellationToken = default, Action<long, long> progressCallback = null)
        {
            if (uri == null)
                throw new ArgumentNullException(nameof(uri));
            if (String.IsNullOrWhiteSpace(filePath))
                throw new ArgumentException("File path is required.", nameof(filePath));

            string tempFilePath = filePath + ".download";
            if (File.Exists(tempFilePath))
            {
                File.Delete(tempFilePath);
            }

            if (uri.IsFile)
            {
                long length = new FileInfo(uri.LocalPath).Length;
                if (IsExistingDownloadComplete(filePath, length))
                {
                    progressCallback?.Invoke(length, length);
                    return;
                }
            }
            else
            {
                long? contentLength = await TryGetContentLength(uri, cancellationToken).ConfigureAwait(false);
                if (contentLength.HasValue && IsExistingDownloadComplete(filePath, contentLength.Value))
                {
                    progressCallback?.Invoke(contentLength.Value, contentLength.Value);
                    return;
                }
            }

            using (var fileStream = File.Create(tempFilePath))
            {
                await DownloadFileAsync(uri, fileStream, cancellationToken, progressCallback).ConfigureAwait(false);
            }

            if (File.Exists(filePath))
            {
                File.Delete(filePath);
            }
            File.Move(tempFilePath, filePath);
        }

        private async Task<long?> TryGetContentLength(Uri uri, CancellationToken cancellationToken)
        {
            try
            {
                using (var request = new HttpRequestMessage(HttpMethod.Head, uri))
                using (var response = await _httpClient.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, cancellationToken).ConfigureAwait(false))
                {
                    if (!response.IsSuccessStatusCode)
                        return null;

                    return response.Content.Headers.ContentLength;
                }
            }
            catch (HttpRequestException)
            {
                return null;
            }
            catch (TaskCanceledException) when (!cancellationToken.IsCancellationRequested)
            {
                return null;
            }
        }

        private static bool IsExistingDownloadComplete(string filePath, long expectedBytes)
        {
            if (expectedBytes <= 0 || !File.Exists(filePath))
                return false;

            return new FileInfo(filePath).Length == expectedBytes;
        }
    }
}
