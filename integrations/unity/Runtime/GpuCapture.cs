using System;
using UnityEngine.Experimental.Rendering;

namespace RenderDocX
{
    /// <summary>Optional scoped access to Unity's experimental external GPU profiler API.</summary>
    public sealed class GpuCapture : IDisposable
    {
        private bool active;
        private static bool captureActive;

        private GpuCapture() { active = true; }

        /// <summary>Call on the Unity main thread around the rendering work to capture.</summary>
        public static bool TryBegin(out GpuCapture capture)
        {
            capture = null;
            // IsAttached can indicate any supported external profiler, not only RenderDoc.
            if (captureActive || !ExternalGPUProfiler.IsAttached()) return false;
            ExternalGPUProfiler.BeginGPUCapture();
            captureActive = true;
            capture = new GpuCapture();
            return true;
        }

        public void Dispose()
        {
            if (!active) return;
            active = false;
            try { ExternalGPUProfiler.EndGPUCapture(); }
            finally { captureActive = false; }
        }
    }
}
