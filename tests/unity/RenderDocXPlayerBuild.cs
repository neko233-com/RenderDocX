using System;
using UnityEditor;
using UnityEditor.Build.Reporting;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Rendering;

// Executed only in a disposable project. The scene and player are test artifacts.
public static class RenderDocXPlayerBuild
{
    public static void Build()
    {
        var args = Environment.GetCommandLineArgs();
        var index = Array.IndexOf(args, "-buildOutput");
        if (index < 0 || index + 1 >= args.Length) throw new InvalidOperationException("Missing -buildOutput");
        GraphicsSettings.renderPipelineAsset = null;
        var currentQuality = QualitySettings.GetQualityLevel();
        for (var i = 0; i < QualitySettings.names.Length; i++)
        {
            QualitySettings.SetQualityLevel(i, false);
            QualitySettings.renderPipeline = null;
        }
        QualitySettings.SetQualityLevel(currentQuality, false);
        var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
        var camera = new GameObject("RenderDocX fixture camera");
        camera.AddComponent<Camera>().backgroundColor = new Color(0.2f, 0.4f, 0.8f, 1f);
        camera.AddComponent<RenderDocXPlayerFixture>();
        var cube = GameObject.CreatePrimitive(PrimitiveType.Cube);
        cube.transform.position = new Vector3(0, 0, 4);
        const string scenePath = "Assets/RenderDocXPlayerFixture/Fixture.unity";
        if (!EditorSceneManager.SaveScene(scene, scenePath)) throw new InvalidOperationException("Could not save test scene");
        var options = new BuildPlayerOptions {
            scenes = new[] { scenePath }, locationPathName = args[index + 1],
            target = BuildTarget.StandaloneWindows64, options = BuildOptions.Development
        };
        var report = BuildPipeline.BuildPlayer(options);
        if (report.summary.result != BuildResult.Succeeded)
            throw new InvalidOperationException("Fixture player build failed: " + report.summary.result);
        Debug.Log("RENDERDOCX_PLAYER_BUILT " + report.summary.outputPath);
    }
}
