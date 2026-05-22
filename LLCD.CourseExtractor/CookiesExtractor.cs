using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using System.Data.SQLite;
using System.Security.Cryptography;
using Newtonsoft.Json.Linq;
using Org.BouncyCastle.Crypto;
using Org.BouncyCastle.Crypto.Engines;
using Org.BouncyCastle.Crypto.Modes;
using Org.BouncyCastle.Crypto.Parameters;
using System.IO;
using Serilog;
using System.Security;

namespace LLCD.CourseExtractor
{
    class CookiesExtractor
    {
        private readonly string _hostName;
        internal CookiesExtractor(string hostName)
        {
            _hostName = hostName;
        }

        internal List<DBCookie> ReadChromeCookies() => ReadChromiumCookies(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData) + @"\Google\Chrome\User Data");

        internal List<DBCookie> ReadEdgeCookies() => ReadChromiumCookies(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData) + @"\Microsoft\Edge\User Data");

        internal List<DBCookie> ReadFirefoxCookies()
        {
            var cookies = new List<DBCookie>();
            string profilesPath = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData) + @"\Mozilla\Firefox\Profiles";
            string defaultProfilePath = Directory.EnumerateDirectories(profilesPath).OrderByDescending(dir => Directory.GetLastWriteTime(dir)).First();
            string dbPath = Path.Combine(defaultProfilePath, "cookies.sqlite");

            var connectionString = "Data Source=" + dbPath + ";pooling=false";

            using (var conn = new SQLiteConnection(connectionString))
            using (var cmd = conn.CreateCommand())
            {
                var prm = cmd.CreateParameter();
                prm.ParameterName = "hostName";
                prm.Value = _hostName;
                cmd.Parameters.Add(prm);

                cmd.CommandText = "SELECT name,value FROM moz_cookies WHERE host = @hostName";

                conn.Open();
                using (var reader = cmd.ExecuteReader())
                {
                    while (reader.Read())
                    {
                        cookies.Add(new DBCookie(reader.GetString(0), reader.GetString(1)));
                    }
                }
                conn.Close();
            }
            return cookies;
        }
        private List<DBCookie> ReadChromiumCookies(string profilePath)
        {
            string localStatePath = Path.Combine(profilePath, "Local State");
            if (!File.Exists(localStatePath))
            {
                return new List<DBCookie>();
            }

            string encKey = File.ReadAllText(localStatePath);
            encKey = JObject.Parse(encKey)["os_crypt"]["encrypted_key"].ToString();
            var decodedKey = UnprotectChromiumKey(encKey);

            // Big thanks to https://stackoverflow.com/a/60611673/6481581 for answering how Chrome 80 and up changed the way cookies are encrypted.

            var cookies = new List<DBCookie>();
            foreach (var dbPath in GetChromiumCookieDatabasePaths(profilePath))
            {
                try
                {
                    cookies.AddRange(GetChromeCookiesFromDB(dbPath, decodedKey));
                }
                catch (Exception ex) when (ex is SQLiteException || ex is IOException || ex is UnauthorizedAccessException || ex is SecurityException || ex is CryptographicException)
                {
                    Log.Error(ex, "Failed to read Chromium cookies from {dbPath}", dbPath);
                }
            }

            return cookies
                .Where(cookie => cookie != null && !String.IsNullOrWhiteSpace(cookie.Value))
                .GroupBy(cookie => cookie.Name + "\n" + cookie.Value)
                .Select(group => group.First())
                .ToList();
        }

        private List<DBCookie> GetChromeCookiesFromDB(string dbPath, byte[] decodedKey)
        {
            var cookies = new List<DBCookie>();
            if (!File.Exists(dbPath))
            {
                return cookies;
            }

            string tempDirectory = Path.Combine(Path.GetTempPath(), "LLCD-Cookies-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(tempDirectory);
            string tempDbPath = Path.Combine(tempDirectory, "Cookies");

            try
            {
                CopySqliteDatabaseFiles(dbPath, tempDbPath);
                var connectionString = "Data Source=" + tempDbPath + ";Read Only=True;Pooling=False";

                using (var conn = new SQLiteConnection(connectionString))
                using (var cmd = conn.CreateCommand())
                {
                    cmd.CommandText = "SELECT name,encrypted_value,value FROM cookies WHERE host_key IN (@hostName, @hostNameWithoutDot, @rootHostName)";

                    AddParameter(cmd, "hostName", _hostName);
                    AddParameter(cmd, "hostNameWithoutDot", _hostName.TrimStart('.'));
                    AddParameter(cmd, "rootHostName", ".linkedin.com");

                    conn.Open();
                    using (var reader = cmd.ExecuteReader())
                    {
                        while (reader.Read())
                        {
                            string value = null;
                            if (!reader.IsDBNull(1))
                            {
                                var encryptedData = (byte[])reader[1];
                                value = DecryptChromiumCookie(encryptedData, decodedKey);
                            }
                            if (String.IsNullOrWhiteSpace(value) && !reader.IsDBNull(2))
                            {
                                value = reader.GetString(2);
                            }
                            cookies.Add(new DBCookie(reader.GetString(0), value));
                        }
                    }
                    conn.Close();
                }
            }
            finally
            {
                try
                {
                    Directory.Delete(tempDirectory, true);
                }
                catch (IOException ex)
                {
                    Log.Error(ex, "Failed to delete temporary cookie database copy");
                }
            }
            return cookies;
        }

        private static void AddParameter(SQLiteCommand cmd, string name, string value)
        {
            var prm = cmd.CreateParameter();
            prm.ParameterName = name;
            prm.Value = value;
            cmd.Parameters.Add(prm);
        }

        private static IEnumerable<string> GetChromiumCookieDatabasePaths(string userDataPath)
        {
            var profileDirectories = Directory.EnumerateDirectories(userDataPath)
                .Where(dir => File.Exists(Path.Combine(dir, @"Network\Cookies")) || File.Exists(Path.Combine(dir, "Cookies")))
                .OrderBy(dir => Path.GetFileName(dir).Equals("Default", StringComparison.OrdinalIgnoreCase) ? 0 : 1)
                .ThenByDescending(dir => Directory.GetLastWriteTimeUtc(dir));

            foreach (var profileDirectory in profileDirectories)
            {
                string networkCookies = Path.Combine(profileDirectory, @"Network\Cookies");
                if (File.Exists(networkCookies))
                {
                    yield return networkCookies;
                }

                string legacyCookies = Path.Combine(profileDirectory, "Cookies");
                if (File.Exists(legacyCookies))
                {
                    yield return legacyCookies;
                }
            }
        }

        private static void CopySqliteDatabaseFiles(string sourceDbPath, string destinationDbPath)
        {
            File.Copy(sourceDbPath, destinationDbPath, true);
            CopyIfExists(sourceDbPath + "-wal", destinationDbPath + "-wal");
            CopyIfExists(sourceDbPath + "-shm", destinationDbPath + "-shm");
        }

        private static void CopyIfExists(string sourcePath, string destinationPath)
        {
            if (File.Exists(sourcePath))
            {
                File.Copy(sourcePath, destinationPath, true);
            }
        }

        private static byte[] UnprotectChromiumKey(string encryptedKey)
        {
            byte[] keyBytes = Convert.FromBase64String(encryptedKey);
            if (keyBytes.Length > 5)
            {
                keyBytes = keyBytes.Skip(5).ToArray();
            }

            try
            {
                return ProtectedData.Unprotect(keyBytes, null, DataProtectionScope.CurrentUser);
            }
            catch (CryptographicException)
            {
                return ProtectedData.Unprotect(keyBytes, null, DataProtectionScope.LocalMachine);
            }
        }

        private string DecryptChromiumCookie(byte[] encryptedData, byte[] decodedKey)
        {
            if (encryptedData == null || encryptedData.Length == 0)
            {
                return null;
            }

            if (encryptedData.Length > 3 && encryptedData[0] == 'v' && encryptedData[1] == '1')
            {
                return DecryptWithKey(encryptedData, decodedKey, 3);
            }

            try
            {
                return Encoding.Default.GetString(ProtectedData.Unprotect(encryptedData, null, DataProtectionScope.CurrentUser));
            }
            catch (CryptographicException)
            {
                return null;
            }
        }

        private string DecryptWithKey(byte[] message, byte[] key, int nonSecretPayloadLength)
        {
            const int KEY_BIT_SIZE = 256;
            const int MAC_BIT_SIZE = 128;
            const int NONCE_BIT_SIZE = 96;

            if (key == null || key.Length != KEY_BIT_SIZE / 8)
                throw new ArgumentException(String.Format("Key needs to be {0} bit!", KEY_BIT_SIZE), "key");
            if (message == null || message.Length == 0)
                throw new ArgumentException("Message required!", "message");

            using (var cipherStream = new MemoryStream(message))
            using (var cipherReader = new BinaryReader(cipherStream))
            {
                var nonSecretPayload = cipherReader.ReadBytes(nonSecretPayloadLength);
                var nonce = cipherReader.ReadBytes(NONCE_BIT_SIZE / 8);
                var cipher = new GcmBlockCipher(new AesEngine());
                var parameters = new AeadParameters(new KeyParameter(key), MAC_BIT_SIZE, nonce);
                cipher.Init(false, parameters);
                var cipherText = cipherReader.ReadBytes(message.Length);
                var plainText = new byte[cipher.GetOutputSize(cipherText.Length)];
                try
                {
                    var len = cipher.ProcessBytes(cipherText, 0, cipherText.Length, plainText, 0);
                    cipher.DoFinal(plainText, len);
                }
                catch (InvalidCipherTextException)
                {
                    return null;
                }
                return Encoding.Default.GetString(plainText);
            }
        }
    }
}
