using System;
using System.Collections;
using UnityEngine;
using UnityEngine.Experimental.Rendering;

namespace RenderDocX
{
    public enum FrameCaptureStart
    {
        Scheduled,
        NotPlaying,
        BatchMode,
        ProfilerNotAttached,
        Busy
    }

    public enum FrameCaptureResult
    {
        Ended,
        ProfilerDetached,
        Cancelled,
        Failed
    }

    /// <summary>Explicitly schedule one Game frame using Unity's external GPU profiler.</summary>
    public static class FrameCapture
    {
        private static FrameCaptureRunner pending;

        /// <summary>
        /// Call on the Unity main thread. Ended means the capture scope ended; it does not
        /// guarantee that the profiler wrote a file or that the profiler is RenderDoc.
        /// </summary>
        public static FrameCaptureStart Schedule(Action<FrameCaptureResult> completed = null)
        {
            if (!Application.isPlaying) return FrameCaptureStart.NotPlaying;
            // Unity documents that WaitForEndOfFrame is not called in Editor batch mode.
#if UNITY_EDITOR
            if (Application.isBatchMode) return FrameCaptureStart.BatchMode;
#endif
            if (pending != null || GpuCapture.IsActive) return FrameCaptureStart.Busy;
            if (!ExternalGPUProfiler.IsAttached()) return FrameCaptureStart.ProfilerNotAttached;

            var host = new GameObject("RenderDocX Frame Capture") { hideFlags = HideFlags.HideAndDontSave };
            UnityEngine.Object.DontDestroyOnLoad(host);
            try
            {
                pending = host.AddComponent<FrameCaptureRunner>();
                pending.Begin(completed);
                return FrameCaptureStart.Scheduled;
            }
            catch
            {
                pending = null;
                UnityEngine.Object.Destroy(host);
                throw;
            }
        }

        /// <summary>Cancel a pending capture on the Unity main thread.</summary>
        public static bool Cancel()
        {
            if (pending == null) return false;
            pending.Cancel();
            return true;
        }

        internal static void Clear(FrameCaptureRunner runner)
        {
            if (pending == runner) pending = null;
        }
    }

    internal sealed class FrameCaptureRunner : MonoBehaviour
    {
        private Action<FrameCaptureResult> completed;
        private GpuCapture scope;
        private bool finished;

        internal void Begin(Action<FrameCaptureResult> callback)
        {
            completed = callback;
            StartCoroutine(Capture());
        }

        private IEnumerator Capture()
        {
            // Start after this frame's rendering, then close after the next frame.
            // Starting from Update can reach the render thread after that frame's
            // camera work has already been submitted.
            yield return new WaitForEndOfFrame();
            bool started = false;
            bool failed = false;
            try { started = GpuCapture.TryBegin(out scope); }
            catch (Exception error)
            {
                Debug.LogException(error);
                failed = true;
            }
            if (failed) { Finish(FrameCaptureResult.Failed); yield break; }
            if (!started)
            {
                Finish(FrameCaptureResult.ProfilerDetached);
                yield break;
            }
            // A second WaitForEndOfFrame can resume in the same frame. Cross an
            // Update boundary first so one complete rendered frame is enclosed.
            yield return null;
            yield return new WaitForEndOfFrame();
            Finish(FrameCaptureResult.Ended);
        }

        internal void Cancel() { Finish(FrameCaptureResult.Cancelled); }
        private void OnDisable() { Finish(FrameCaptureResult.Cancelled); }
        private void OnDestroy() { Finish(FrameCaptureResult.Cancelled); }

        private void Finish(FrameCaptureResult result)
        {
            if (finished) return;
            finished = true;
            try
            {
                if (scope != null) scope.Dispose();
            }
            catch (Exception error)
            {
                Debug.LogException(error);
                result = FrameCaptureResult.Failed;
            }
            finally
            {
                scope = null;
                FrameCapture.Clear(this);
            }
            try { if (completed != null) completed(result); }
            catch (Exception error) { Debug.LogException(error); }
            if (gameObject != null) Destroy(gameObject);
        }
    }
}
