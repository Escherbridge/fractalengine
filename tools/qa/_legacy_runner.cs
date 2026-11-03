using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

public sealed class QaLegacyHelperResult
{
    public string Text { get; set; }
    public int ExitCode { get; set; }
}

// One owned helper operation; see AGENTS.md, Disposable legacy adoption fixture.
public sealed class QaLegacyHelperOperation
{
    private readonly Process process;
    private readonly SafeFileHandle lease;
    private readonly SafeFileHandle executable;
    private readonly Task<bool> started;
    private readonly Stopwatch clock = Stopwatch.StartNew();
    private int stopping;
    private int released;
    private bool leaseHeld;
    private bool executableHeld;

    private QaLegacyHelperOperation(ProcessStartInfo start, SafeFileHandle lease, SafeFileHandle executable)
    {
        this.lease = lease;
        this.executable = executable;
        try
        {
            lease.DangerousAddRef(ref leaseHeld);
            executable.DangerousAddRef(ref executableHeld);
            process = new Process { StartInfo = start };
            started = Task.Run(() => process.Start());
        }
        catch
        {
            Release();
            throw new InvalidOperationException("helper_start_failed");
        }
    }

    private void Release()
    {
        if (Interlocked.Exchange(ref released, 1) != 0) return;
        if (executableHeld) executable.DangerousRelease();
        if (leaseHeld) lease.DangerousRelease();
    }

    private static async Task<byte[]> Collect(Stream stream, int limit)
    {
        using (var bytes = new MemoryStream())
        {
            var buffer = new byte[4096];
            while (true)
            {
                int count = await stream.ReadAsync(buffer, 0, buffer.Length).ConfigureAwait(false);
                if (count == 0) return bytes.ToArray();
                if (count > limit - bytes.Length)
                    throw new InvalidOperationException("helper_output_refused");
                bytes.Write(buffer, 0, count);
            }
        }
    }

    private static async Task Write(Stream stream, byte[] input)
    {
        await stream.WriteAsync(input, 0, input.Length).ConfigureAwait(false);
        await stream.FlushAsync().ConfigureAwait(false);
        stream.Close();
    }

    private async Task<QaLegacyHelperResult> Execute(byte[] input)
    {
        if (!await started.ConfigureAwait(false))
            throw new InvalidOperationException("helper_start_failed");
        if (Volatile.Read(ref stopping) != 0)
            throw new InvalidOperationException("helper_deadline");
        var output = Collect(process.StandardOutput.BaseStream, 4194304);
        var errors = Collect(process.StandardError.BaseStream, 0);
        var writing = Write(process.StandardInput.BaseStream, input);
        var exited = Task.Run(() => process.WaitForExit());
        Task[] pending = { output, errors, writing, exited };
        foreach (var task in pending) Observe(task);
        while (pending.Length != 0)
        {
            var completed = await Task.WhenAny(pending).ConfigureAwait(false);
            await completed.ConfigureAwait(false);
            pending = Array.FindAll(pending, task => task != completed);
        }
        var result = new QaLegacyHelperResult {
            Text = new UTF8Encoding(false, true).GetString(output.Result), ExitCode = process.ExitCode
        };
        Close();
        Release();
        return result;
    }

    private static void Observe(Task task)
    {
        task.ContinueWith(failed => { var ignored = failed.Exception; },
            TaskContinuationOptions.OnlyOnFaulted);
    }

    private void Close()
    {
        process.StandardInput.Close();
        process.StandardOutput.Close();
        process.StandardError.Close();
        process.Dispose();
    }

    private async Task Terminate()
    {
        try
        {
            if (await started.ConfigureAwait(false))
            {
                if (!process.HasExited) process.Kill();
                process.WaitForExit();
                Close();
            }
            else process.Dispose();
            Release();
        }
        catch
        {
            // An unconfirmed lifetime retains both SafeHandle references until host exit.
        }
    }

    private async Task<QaLegacyHelperResult> Complete(byte[] input)
    {
        string failure = "helper_io_refused";
        var body = Task.Run(() => Execute(input));
        int remaining = Math.Max(0, 35000 - (int)Math.Min(35000, clock.ElapsedMilliseconds));
        if (await Task.WhenAny(body, Task.Delay(remaining)).ConfigureAwait(false) == body)
        {
            try
            {
                return await body.ConfigureAwait(false);
            }
            catch (Exception error)
            {
                if (error is InvalidOperationException && error.Message == "helper_output_refused")
                    failure = "helper_output_refused";
            }
        }
        else failure = "helper_deadline";

        Interlocked.Exchange(ref stopping, 1);
        var cleanup = Task.Run(() => Terminate());
        if (await Task.WhenAny(cleanup, Task.Delay(5000)).ConfigureAwait(false) != cleanup ||
            Volatile.Read(ref released) == 0)
            failure = "helper_lifetime_unknown";
        // Observe a later pipe/start failure without waiting on inherited pipe handles.
        Observe(body);
        throw new InvalidOperationException(failure);
    }

    public static QaLegacyHelperResult Run(string helper, string mode, string directory, string json,
        SafeFileHandle lease, SafeFileHandle executable)
    {
        var input = new UTF8Encoding(false, true).GetBytes(json);
        if (input.Length > 65536) throw new InvalidOperationException("request_overflow");
        var start = new ProcessStartInfo(helper, mode) {
            WorkingDirectory = directory, UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true
        };
        return new QaLegacyHelperOperation(start, lease, executable).Complete(input).GetAwaiter().GetResult();
    }
}
