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
        if (File.Exists(stop) || Time.realtimeSinceStartup >= deadline)
        {
            Application.Quit(File.Exists(stop) ? 0 : 1);
            return;
        }
        if (!File.Exists(request)) return;
        File.Delete(request);
        RenderDocX.GpuCapture capture;
        if (!RenderDocX.GpuCapture.TryBegin(out capture))
        {
            File.WriteAllText(request + ".result", "not-attached");
            return;
        }
        try
        {
            RenderDocX.GpuCapture nested;
            if (RenderDocX.GpuCapture.TryBegin(out nested))
            {
                nested.Dispose();
                File.WriteAllText(request + ".result", "nested-capture-accepted");
                return;
            }
            fixtureCamera.Render();
        }
        finally
        {
            capture.Dispose();
        }
        File.WriteAllText(request + ".result", "capture-ended");
    }

    private void OnDestroy()
    {
        if (target != null) { target.Release(); Destroy(target); }
    }
}
