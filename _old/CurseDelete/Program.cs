using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Threading;
using System.Threading.Tasks;

internal static class Program
{
    private const string AppName = "CursedDelete";
    private const string AppVersion = "1.0.0";
    private const string AppInfo = AppName + " " + AppVersion + " | Recursive fast deleter";

    // -------------------------------------------------------------------------
    // P/Invoke – Unix effective UID (geteuid). Ignored on Windows at runtime.
    // -------------------------------------------------------------------------
    [DllImport("libc", EntryPoint = "geteuid", SetLastError = false)]
    [SupportedOSPlatform("linux")]
    [SupportedOSPlatform("osx")]
    private static extern uint GetEffectiveUserId();

    // -------------------------------------------------------------------------
    // Types
    // -------------------------------------------------------------------------

    private sealed class Options
    {
        public required string Target { get; init; }
        public bool Recurse { get; init; }
        public bool Verbose { get; init; }
        public bool Force { get; init; }
        public int DegreeOfParallelism { get; init; } = Math.Clamp(Environment.ProcessorCount * 2, 4, 64);
    }

    private sealed class DeleteResult
    {
        private int _filesDeleted;
        private int _directoriesDeleted;
        private int _failures;

        public int FilesDeleted => _filesDeleted;
        public int DirectoriesDeleted => _directoriesDeleted;
        public int Failures => _failures;

        public void IncrementFiles() => Interlocked.Increment(ref _filesDeleted);
        public void IncrementDirectories() => Interlocked.Increment(ref _directoriesDeleted);
        public void IncrementFailures() => Interlocked.Increment(ref _failures);
    }

    // Typed error categories for verbose diagnostics.
    private enum DeleteError
    {
        AccessDenied,
        PathTooLong,
        DirectoryNotEmpty,
        FileInUse,
        NotFound,
        Other
    }

    // -------------------------------------------------------------------------
    // Entry point
    // -------------------------------------------------------------------------

    private static int Main(string[] args)
    {
        if (args.Length == 0)
        {
            PrintUsage();
            return 1;
        }

        if (IsVersionCommand(args))
        {
            PrintVersion();
            return 0;
        }

        try
        {
            var options = ParseArgs(args);

            // -force requires elevation so we can take ownership and reset ACLs
            // (Windows) or chmod locked paths as root (Unix).
            if (options.Force && !IsElevated())
            {
                var platform = OperatingSystem.IsWindows() ? "Administrator" : "root (sudo)";
                Console.Error.WriteLine(
                    $"Error: -force requires {platform} privileges " +
                    $"in order to take ownership and reset permissions on locked paths. " +
                    $"Please re-run with elevated privileges.");
                return 1;
            }

            var stopwatch = Stopwatch.StartNew();
            var result = Execute(options);
            stopwatch.Stop();

            Console.WriteLine(
                $"Completed. Files deleted: {result.FilesDeleted:N0}, " +
                $"Directories deleted: {result.DirectoriesDeleted:N0}, " +
                $"Failures: {result.Failures:N0}, " +
                $"Elapsed: {stopwatch.Elapsed}");

            return result.Failures == 0 ? 0 : 2;
        }
        catch (ArgumentException ex)
        {
            Console.Error.WriteLine(ex.Message);
            PrintUsage();
            return 1;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Fatal error: {ex.Message}");
            return 99;
        }
    }

    // -------------------------------------------------------------------------
    // Version / usage
    // -------------------------------------------------------------------------

    private static bool IsVersionCommand(string[] args)
    {
        if (args.Length != 1) return false;

        var arg = args[0].Trim();
        return arg.Equals("version", StringComparison.OrdinalIgnoreCase)
            || arg.Equals("--version", StringComparison.OrdinalIgnoreCase)
            || arg.Equals("-version", StringComparison.OrdinalIgnoreCase)
            || arg.Equals("/version", StringComparison.OrdinalIgnoreCase)
            || arg.Equals("-vinfo", StringComparison.OrdinalIgnoreCase);
    }

    private static void PrintVersion()
    {
        Console.WriteLine(AppInfo);
        Console.WriteLine($"OS           : {GetOsName()}");
        Console.WriteLine($"64-bit       : {Environment.Is64BitProcess}");
        Console.WriteLine($".NET         : {Environment.Version}");
        Console.WriteLine($"Machine      : {Environment.MachineName}");
        Console.WriteLine($"User         : {Environment.UserName}");
        Console.WriteLine($"Elevated     : {IsElevated()}");
    }

    private static void PrintUsage()
    {
        Console.WriteLine(AppName);
        Console.WriteLine();
        Console.WriteLine("Usage:");
        Console.WriteLine("  cursedelete <path>");
        Console.WriteLine("  cursedelete <path> -recurse");
        Console.WriteLine("  cursedelete <path> -force");
        Console.WriteLine("  cursedelete <path> -recurse -force");
        Console.WriteLine("  cursedelete version");
        Console.WriteLine();
        Console.WriteLine("Examples:");
        Console.WriteLine(@"  cursedelete C:\Temp\OldFolder");
        Console.WriteLine(@"  cursedelete C:\Temp\file.txt");
        Console.WriteLine(@"  cursedelete C:\Downloads\*.md");
        Console.WriteLine(@"  cursedelete C:\Downloads\*.md -recurse");
        Console.WriteLine(@"  cursedelete \\server\share\drop\*.log -recurse -force");
        Console.WriteLine(@"  cursedelete /mnt/share/logs/*.tmp -recurse -force");
        Console.WriteLine(@"  cursedelete version");
        Console.WriteLine();
        Console.WriteLine("Flags:");
        Console.WriteLine("  -recurse        Recurse into subdirectories for wildcard deletes.");
        Console.WriteLine("  -force          Take ownership, reset ACLs, clear attributes, retry.");
        Console.WriteLine("                  Requires Administrator (Windows) or root/sudo (Unix).");
        Console.WriteLine("  -v              Verbose logging with typed error diagnostics.");
        Console.WriteLine("  -dop <number>   Max parallel file deletions.");
        Console.WriteLine();
        Console.WriteLine("Notes:");
        Console.WriteLine("  Reparse points (junctions/symlinks) are never recursed into.");
        Console.WriteLine("  Wildcard patterns match files only, not directory names.");
        Console.WriteLine("  -force does not bypass OS ownership or access control boundaries;");
        Console.WriteLine("  it operates within the authority of the current elevated user.");
    }

    // -------------------------------------------------------------------------
    // Argument parsing
    // -------------------------------------------------------------------------

    private static Options ParseArgs(string[] args)
    {
        string? target = null;
        bool recurse = false;
        bool verbose = false;
        bool force = false;
        int? dop = null;

        for (int i = 0; i < args.Length; i++)
        {
            var arg = args[i];

            if (arg.Equals("-recurse", StringComparison.OrdinalIgnoreCase) ||
                arg.Equals("/recurse", StringComparison.OrdinalIgnoreCase))
            {
                recurse = true;
                continue;
            }

            if (arg.Equals("-force", StringComparison.OrdinalIgnoreCase) ||
                arg.Equals("/force", StringComparison.OrdinalIgnoreCase))
            {
                force = true;
                continue;
            }

            if (arg.Equals("-v", StringComparison.OrdinalIgnoreCase) ||
                arg.Equals("--verbose", StringComparison.OrdinalIgnoreCase) ||
                arg.Equals("/v", StringComparison.OrdinalIgnoreCase))
            {
                verbose = true;
                continue;
            }

            if (arg.Equals("-dop", StringComparison.OrdinalIgnoreCase) ||
                arg.Equals("/dop", StringComparison.OrdinalIgnoreCase))
            {
                if (i + 1 >= args.Length || !int.TryParse(args[++i], out var parsed) || parsed < 1)
                    throw new ArgumentException("Invalid value for -dop.");

                dop = parsed;
                continue;
            }

            if (target is null)
            {
                target = arg;
                continue;
            }

            throw new ArgumentException($"Unknown argument: {arg}");
        }

        if (string.IsNullOrWhiteSpace(target))
            throw new ArgumentException("No target path was supplied.");

        return new Options
        {
            Target = target,
            Recurse = recurse,
            Verbose = verbose,
            Force = force,
            DegreeOfParallelism = dop ?? Math.Clamp(Environment.ProcessorCount, 2, 16)
        };
    }

    // -------------------------------------------------------------------------
    // Execution dispatch
    // -------------------------------------------------------------------------

    private static DeleteResult Execute(Options options)
    {
        var target = options.Target.Trim();

        if (ContainsWildcard(target))
            return DeleteByPattern(target, options);

        if (File.Exists(target))
        {
            var result = new DeleteResult();

            if (TryDeleteFile(target, options, out _))
                result.IncrementFiles();
            else
                result.IncrementFailures();

            return result;
        }

        if (Directory.Exists(target))
            return DeleteDirectoryTree(target, options);

        throw new ArgumentException($"Target not found: {target}");
    }

    // -------------------------------------------------------------------------
    // Wildcard delete
    //
    // Note: matches files only. Directories whose names match the pattern are
    // intentionally excluded – pass a plain directory path to delete a tree.
    //
    // When -recurse is set, uses an explicit stack walk (same as DeleteDirectoryTree)
    // so that reparse points (junctions/symlinks) are never traversed. Using
    // SearchOption.AllDirectories via System.IO would bypass our IsReparsePoint
    // guard entirely, which is the bug this fixes.
    // -------------------------------------------------------------------------

    private static DeleteResult DeleteByPattern(string patternPath, Options options)
    {
        var result = new DeleteResult();

        var directoryPart = Path.GetDirectoryName(patternPath);
        var filePattern = Path.GetFileName(patternPath);

        if (string.IsNullOrWhiteSpace(filePattern))
            throw new ArgumentException("Invalid wildcard path.");

        var baseDirectory = string.IsNullOrWhiteSpace(directoryPart)
            ? Directory.GetCurrentDirectory()
            : directoryPart;

        if (!Directory.Exists(baseDirectory))
            throw new ArgumentException($"Directory not found: {baseDirectory}");

        var queue = new BlockingCollection<string>(boundedCapacity: options.DegreeOfParallelism * 4);

        // Start consumers before enumeration so deletion begins immediately.
        var workers = new Task[options.DegreeOfParallelism];
        for (int w = 0; w < workers.Length; w++)
        {
            workers[w] = Task.Run(() =>
            {
                foreach (var file in queue.GetConsumingEnumerable())
                {
                    if (TryDeleteFile(file, options, out _))
                        result.IncrementFiles();
                    else
                        result.IncrementFailures();
                }
            });
        }

        try
        {
            if (options.Recurse)
            {
                // Explicit stack walk so we can apply the reparse point guard,
                // which SearchOption.AllDirectories would silently bypass.
                var stack = new Stack<string>();
                stack.Push(baseDirectory);

                while (stack.Count > 0)
                {
                    var current = stack.Pop();

                    try
                    {
                        foreach (var file in Directory.EnumerateFiles(current, filePattern, SearchOption.TopDirectoryOnly))
                            queue.Add(file);
                    }
                    catch (Exception ex)
                    {
                        if (options.Verbose)
                            Console.Error.WriteLine($"Enumerate files failed: {current} | {ex.Message}");

                        result.IncrementFailures();
                    }

                    try
                    {
                        foreach (var dir in Directory.EnumerateDirectories(current))
                        {
                            if (OperatingSystem.IsWindows() && IsReparsePoint(dir))
                            {
                                if (options.Verbose)
                                    Console.Error.WriteLine($"Skipped reparse point : {dir}");

                                continue;
                            }

                            stack.Push(dir);
                        }
                    }
                    catch (Exception ex)
                    {
                        if (options.Verbose)
                            Console.Error.WriteLine($"Enumerate dirs failed : {current} | {ex.Message}");

                        result.IncrementFailures();
                    }
                }
            }
            else
            {
                foreach (var file in Directory.EnumerateFiles(baseDirectory, filePattern, SearchOption.TopDirectoryOnly))
                    queue.Add(file);
            }
        }
        catch (Exception ex)
        {
            if (options.Verbose)
                Console.Error.WriteLine($"Enumerate failed      : {baseDirectory} | {ex.Message}");

            result.IncrementFailures();
        }
        finally
        {
            queue.CompleteAdding();
        }

        Task.WaitAll(workers);
        return result;
    }

    // -------------------------------------------------------------------------
    // Directory tree delete
    //
    // Strategy:
    //   1. Walk tree with explicit stack; reparse points are skipped (never
    //      recursed into) to avoid walking outside the intended root or looping.
    //   2. Files are fed into a bounded producer/consumer queue; workers delete
    //      in parallel as enumeration proceeds rather than after it completes.
    //      This prevents unbounded memory growth on multi-million-file trees.
    //   3. Directories are deleted deepest-first (reverse discovery order)
    //      after all file workers have drained.
    // -------------------------------------------------------------------------

    private static DeleteResult DeleteDirectoryTree(string rootPath, Options options)
    {
        var result = new DeleteResult();
        var directories = new List<string>(capacity: 1024);
        var fileQueue = new BlockingCollection<string>(boundedCapacity: options.DegreeOfParallelism * 8);
        var stack = new Stack<string>();

        stack.Push(rootPath);

        // Start consumers before enumeration so deletion begins immediately.
        var workers = new Task[options.DegreeOfParallelism];
        for (int w = 0; w < workers.Length; w++)
        {
            workers[w] = Task.Run(() =>
            {
                foreach (var file in fileQueue.GetConsumingEnumerable())
                {
                    if (TryDeleteFile(file, options, out _))
                        result.IncrementFiles();
                    else
                        result.IncrementFailures();
                }
            });
        }

        // Producer: walk tree and stream files to workers.
        while (stack.Count > 0)
        {
            var current = stack.Pop();
            directories.Add(current);

            try
            {
                foreach (var file in Directory.EnumerateFiles(current))
                    fileQueue.Add(file);
            }
            catch (Exception ex)
            {
                if (options.Verbose)
                    Console.Error.WriteLine($"Enumerate files failed: {current} | {ex.Message}");

                result.IncrementFailures();
            }

            try
            {
                foreach (var dir in Directory.EnumerateDirectories(current))
                {
                    // Never recurse into reparse points (junctions, symlinks).
                    // On read failure, IsReparsePoint returns true (safe default).
                    if (OperatingSystem.IsWindows() && IsReparsePoint(dir))
                    {
                        if (options.Verbose)
                            Console.Error.WriteLine($"Skipped reparse point : {dir}");

                        continue;
                    }

                    stack.Push(dir);
                }
            }
            catch (Exception ex)
            {
                if (options.Verbose)
                    Console.Error.WriteLine($"Enumerate dirs failed : {current} | {ex.Message}");

                result.IncrementFailures();
            }
        }

        fileQueue.CompleteAdding();
        Task.WaitAll(workers);

        // Delete directories deepest-first.
        for (int i = directories.Count - 1; i >= 0; i--)
        {
            if (TryDeleteDirectory(directories[i], options, out _))
                result.IncrementDirectories();
            else
                result.IncrementFailures();
        }

        return result;
    }

    // -------------------------------------------------------------------------
    // Delete helpers
    // -------------------------------------------------------------------------

    private static bool ContainsWildcard(string path)
        => path.IndexOfAny(['*', '?']) >= 0;

    private static bool TryDeleteFile(string path, Options options, out string? error)
    {
        try
        {
            PrepareWindowsAttributes(path, isDirectory: false);
            File.Delete(path);

            if (options.Verbose)
                Console.WriteLine($"Deleted file          : {path}");

            error = null;
            return true;
        }
        catch (Exception firstEx)
        {
            if (options.Force)
            {
                bool unlocked = ForceUnlockPath(path, isDirectory: false, options);

                if (options.Verbose && !unlocked)
                    Console.Error.WriteLine($"Unlock incomplete     : {path} (attempting delete anyway)");

                try
                {
                    File.Delete(path);

                    if (options.Verbose)
                        Console.WriteLine($"Deleted file (forced) : {path}");

                    error = null;
                    return true;
                }
                catch (Exception secondEx)
                {
                    LogDeleteFailure(path, isDirectory: false, secondEx, options);
                    error = secondEx.Message;
                    return false;
                }
            }

            LogDeleteFailure(path, isDirectory: false, firstEx, options);
            error = firstEx.Message;
            return false;
        }
    }

    private static bool TryDeleteDirectory(string path, Options options, out string? error)
    {
        try
        {
            PrepareWindowsAttributes(path, isDirectory: true);
            Directory.Delete(path, recursive: false);

            if (options.Verbose)
                Console.WriteLine($"Deleted dir           : {path}");

            error = null;
            return true;
        }
        catch (Exception firstEx)
        {
            if (options.Force)
            {
                bool unlocked = ForceUnlockPath(path, isDirectory: true, options);

                if (options.Verbose && !unlocked)
                    Console.Error.WriteLine($"Unlock incomplete     : {path} (attempting delete anyway)");

                try
                {
                    Directory.Delete(path, recursive: false);

                    if (options.Verbose)
                        Console.WriteLine($"Deleted dir (forced)  : {path}");

                    error = null;
                    return true;
                }
                catch (Exception secondEx)
                {
                    LogDeleteFailure(path, isDirectory: true, secondEx, options);
                    error = secondEx.Message;
                    return false;
                }
            }

            LogDeleteFailure(path, isDirectory: true, firstEx, options);
            error = firstEx.Message;
            return false;
        }
    }

    // Typed verbose failure output makes diagnostics actionable.
    private static void LogDeleteFailure(string path, bool isDirectory, Exception ex, Options options)
    {
        if (!options.Verbose) return;

        var category = ClassifyError(ex);
        var kind = isDirectory ? "dir " : "file";
        Console.Error.WriteLine($"Failed {kind} [{category,-16}]: {path} | {ex.Message}");
    }

    private static DeleteError ClassifyError(Exception ex) => ex switch
    {
        UnauthorizedAccessException                        => DeleteError.AccessDenied,
        PathTooLongException                               => DeleteError.PathTooLong,
        IOException ioe when ioe.Message.Contains(
            "not empty", StringComparison.OrdinalIgnoreCase) => DeleteError.DirectoryNotEmpty,
        // HResult 0x80070020 = ERROR_SHARING_VIOLATION
        IOException ioe when ioe.HResult == unchecked((int)0x80070020) => DeleteError.FileInUse,
        FileNotFoundException or DirectoryNotFoundException => DeleteError.NotFound,
        _                                                  => DeleteError.Other
    };

    // -------------------------------------------------------------------------
    // Attribute prep – runs on every delete attempt; no elevation required.
    // FileAttributes.Normal is the decisive clear-all for files.
    // -------------------------------------------------------------------------

    private static void PrepareWindowsAttributes(string path, bool isDirectory)
    {
        if (!OperatingSystem.IsWindows()) return;

        try
        {
            if (!isDirectory && File.Exists(path))
            {
                File.SetAttributes(path, FileAttributes.Normal);
            }
            else if (isDirectory && Directory.Exists(path))
            {
                var attrs = File.GetAttributes(path);
                if ((attrs & FileAttributes.ReadOnly) != 0)
                    File.SetAttributes(path, attrs & ~FileAttributes.ReadOnly);
            }
        }
        catch
        {
            // Best-effort; the delete attempt will surface any real error.
        }
    }

    // -------------------------------------------------------------------------
    // Force-unlock
    //
    // Returns true only if BOTH ownership and ACL steps succeeded.
    // Callers log the incomplete-unlock warning and attempt the delete anyway –
    // partial unlocks sometimes still get the job done.
    // -------------------------------------------------------------------------

    private static bool ForceUnlockPath(string path, bool isDirectory, Options options)
    {
        if (OperatingSystem.IsWindows())
            return ForceUnlockWindows(path, isDirectory, options);

        if (OperatingSystem.IsLinux() || OperatingSystem.IsMacOS())
            return ForceUnlockUnix(path, isDirectory, options);

        return false;
    }

    // Windows sequence:
    //   1. Take ownership (current user)
    //   2. Disable inheritance, replace ACL with Admins + SYSTEM + current user
    //   3. Clear obstructive attributes (always attempted)
    [SupportedOSPlatform("windows")]
    private static bool ForceUnlockWindows(string path, bool isDirectory, Options options)
    {
        bool ownershipOk = TakeOwnership(path, isDirectory, options);
        bool aclOk = ResetAcl(path, isDirectory, options);
        PrepareWindowsAttributes(path, isDirectory);
        return ownershipOk && aclOk;
    }

    [SupportedOSPlatform("windows")]
    private static bool TakeOwnership(string path, bool isDirectory, Options options)
    {
        try
        {
            var currentUser = WindowsIdentity.GetCurrent().User
                ?? throw new InvalidOperationException("Could not get current user SID.");

            if (!isDirectory && File.Exists(path))
            {
                var fi = new FileInfo(path);
                var security = fi.GetAccessControl();
                security.SetOwner(currentUser);
                fi.SetAccessControl(security);
            }
            else if (isDirectory && Directory.Exists(path))
            {
                var di = new DirectoryInfo(path);
                var security = di.GetAccessControl();
                security.SetOwner(currentUser);
                di.SetAccessControl(security);
            }

            if (options.Verbose)
                Console.WriteLine($"Took ownership        : {path}");

            return true;
        }
        catch (Exception ex)
        {
            if (options.Verbose)
                Console.Error.WriteLine($"TakeOwnership failed  : {path} | {ex.Message}");

            return false;
        }
    }

    [SupportedOSPlatform("windows")]
    private static bool ResetAcl(string path, bool isDirectory, Options options)
    {
        try
        {
            var admins = new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null);
            var system = new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null);
            var currentUser = WindowsIdentity.GetCurrent().User;

            if (!isDirectory && File.Exists(path))
            {
                var security = new FileSecurity();
                // Protect from inheritance; do not preserve any inherited rules.
                security.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);

                security.AddAccessRule(new FileSystemAccessRule(
                    admins, FileSystemRights.FullControl, AccessControlType.Allow));
                security.AddAccessRule(new FileSystemAccessRule(
                    system, FileSystemRights.FullControl, AccessControlType.Allow));

                // Current user gets full control so the subsequent delete succeeds.
                if (currentUser is not null)
                    security.AddAccessRule(new FileSystemAccessRule(
                        currentUser, FileSystemRights.FullControl, AccessControlType.Allow));

                new FileInfo(path).SetAccessControl(security);
            }
            else if (isDirectory && Directory.Exists(path))
            {
                var security = new DirectorySecurity();
                security.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);

                var inherit = InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit;

                security.AddAccessRule(new FileSystemAccessRule(
                    admins, FileSystemRights.FullControl,
                    inherit, PropagationFlags.None, AccessControlType.Allow));
                security.AddAccessRule(new FileSystemAccessRule(
                    system, FileSystemRights.FullControl,
                    inherit, PropagationFlags.None, AccessControlType.Allow));

                if (currentUser is not null)
                    security.AddAccessRule(new FileSystemAccessRule(
                        currentUser, FileSystemRights.FullControl,
                        inherit, PropagationFlags.None, AccessControlType.Allow));

                new DirectoryInfo(path).SetAccessControl(security);
            }

            if (options.Verbose)
                Console.WriteLine($"Reset ACL             : {path}");

            return true;
        }
        catch (Exception ex)
        {
            if (options.Verbose)
                Console.Error.WriteLine($"ResetAcl failed       : {path} | {ex.Message}");

            return false;
        }
    }

    [SupportedOSPlatform("linux")]
    [SupportedOSPlatform("osx")]
    private static bool ForceUnlockUnix(string path, bool isDirectory, Options options)
    {
        try
        {
#if NET6_0_OR_GREATER
            if (!isDirectory && File.Exists(path))
            {
                var mode = File.GetUnixFileMode(path);
                mode |= UnixFileMode.UserWrite;
                File.SetUnixFileMode(path, mode);
            }
            else if (isDirectory && Directory.Exists(path))
            {
                var mode = File.GetUnixFileMode(path);
                mode |= UnixFileMode.UserWrite | UnixFileMode.UserRead | UnixFileMode.UserExecute;
                File.SetUnixFileMode(path, mode);
            }
#endif
            return true;
        }
        catch (Exception ex)
        {
            if (options.Verbose)
                Console.Error.WriteLine($"chmod failed          : {path} | {ex.Message}");

            return false;
        }
    }

    // -------------------------------------------------------------------------
    // Reparse point detection
    //
    // Prevents recursing into junctions/symlinks, which could walk outside the
    // intended root, loop, or delete unexpected content.
    // Returns true on attribute-read failure (safe default = skip the entry).
    // -------------------------------------------------------------------------

    private static bool IsReparsePoint(string path)
    {
        try
        {
            return (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0;
        }
        catch
        {
            return true; // Treat unreadable entries as reparse points to be safe.
        }
    }

    // -------------------------------------------------------------------------
    // Utility
    // -------------------------------------------------------------------------

    private static string GetOsName()
    {
        if (OperatingSystem.IsWindows()) return "Windows";
        if (OperatingSystem.IsLinux())  return "Linux";
        if (OperatingSystem.IsMacOS())  return "macOS";
        return RuntimeInformation.OSDescription;
    }

    // Unix elevation uses geteuid() == 0 via P/Invoke.
    // This correctly handles sudo, setuid binaries, and any case where the
    // effective UID differs from the login name – unlike checking UserName == "root".
    private static bool IsElevated()
    {
        try
        {
            if (OperatingSystem.IsWindows())
            {
                using var identity = WindowsIdentity.GetCurrent();
                var principal = new WindowsPrincipal(identity);
                return principal.IsInRole(WindowsBuiltInRole.Administrator);
            }

            if (OperatingSystem.IsLinux() || OperatingSystem.IsMacOS())
                return GetEffectiveUserId() == 0;

            return false;
        }
        catch
        {
            return false;
        }
    }
}
