using System;
using System.IO;
using UnityEngine;
using UnityEngine.Experimental.Rendering;

// Runtime fixture copied into a disposable Unity project for capture integration tests.
public sealed class RenderDocXPlayerFixture : MonoBehaviour
{
    private string ready;
    private string request;
    private string stop;
    private Camera fixtureCamera;
    private RenderTexture target;
    private float deadline;
    private bool scheduledRendering;

    private void Start()
    {
        ready = Environment.GetEnvironmentVariable("RENDERDOCX_PLAYER_READY");
        request = Environment.GetEnvironmentVariable("RENDERDOCX_PLAYER_CAPTURE");
        stop = Environment.GetEnvironmentVariable("RENDERDOCX_PLAYER_STOP");
        if (string.IsNullOrEmpty(ready) || string.IsNullOrEmpty(request) || string.IsNullOrEmpty(stop))
        {
            Debug.LogError("Missing RenderDocX test synchronization paths");
            Application.Quit(2);
            return;
        }
        Application.runInBackground = true;
        Application.targetFrameRate = 30;
        target = new RenderTexture(64, 64, 24, RenderTextureFormat.ARGB32);
        target.Create();
        fixtureCamera = GetComponent<Camera>();
        fixtureCamera.enabled = false;
        fixtureCamera.targetTexture = target;
        deadline = Time.realtimeSinceStartup + 120;
        using (var process = System.Diagnostics.Process.GetCurrentProcess())
            File.WriteAllText(ready, process.Id + "\n" + SystemInfo.graphicsDeviceType + "\n" + ExternalGPUProfiler.IsAttached());
    }

    private void Update()
    {
        if (string.IsNullOrEmpty(stop)) return;
        if (scheduledRendering) fixtureCamera.Render();
        if (File.Exists(stop) || Time.realtimeSinceStartup >= deadline)
        {
            Application.Quit(File.Exists(stop) ? 0 : 1);
            return;
        }
        if (!File.Exists(request)) return;
        var mode = File.ReadAllText(request).Trim();
        File.Delete(request);
        if (mode == "cancel")
        {
            var callbacks = 0;
            var outcome = RenderDocX.FrameCaptureResult.Failed;
            var started = RenderDocX.FrameCapture.Schedule(value => { callbacks++; outcome = value; });
            var duplicate = RenderDocX.FrameCapture.Schedule();
            var cancelled = RenderDocX.FrameCapture.Cancel();
            var cancelledAgain = RenderDocX.FrameCapture.Cancel();
            WriteResult(
                started == RenderDocX.FrameCaptureStart.Scheduled &&
                duplicate == RenderDocX.FrameCaptureStart.Busy && cancelled && !cancelledAgain &&
                callbacks == 1 && outcome == RenderDocX.FrameCaptureResult.Cancelled
                    ? "cancel-passed" : "cancel-failed");
            return;
        }
        if (mode == "scheduled")
        {
            scheduledRendering = true;
            var scheduled = RenderDocX.FrameCapture.Schedule(result =>
            {
                var report = scheduledRendering;
                scheduledRendering = false;
                if (report) WriteResult("scheduled-" + result);
            });
            if (scheduled != RenderDocX.FrameCaptureStart.Scheduled)
            {
                scheduledRendering = false;
                WriteResult("schedule-" + scheduled);
            }
            else if (RenderDocX.FrameCapture.Schedule() != RenderDocX.FrameCaptureStart.Busy)
            {
                scheduledRendering = false;
                RenderDocX.FrameCapture.Cancel();
                WriteResult("duplicate-accepted");
            }
            return;
        }
        RenderDocX.GpuCapture capture;
        if (!RenderDocX.GpuCapture.TryBegin(out capture))
        {
            WriteResult("not-attached");
            return;
        }
        try
        {
            RenderDocX.GpuCapture nested;
            if (RenderDocX.GpuCapture.TryBegin(out nested))
            {
                nested.Dispose();
                WriteResult("nested-capture-accepted");
                return;
            }
            fixtureCamera.Render();
        }
        finally
        {
            capture.Dispose();
        }
        WriteResult("capture-ended");
    }

    private void WriteResult(string value)
    {
        var result = request + ".result";
        var temporary = result + ".tmp";
        File.WriteAllText(temporary, value);
        File.Move(temporary, result);
    }

    private void OnDestroy()
    {
        if (target != null) { target.Release(); Destroy(target); }
    }
}
