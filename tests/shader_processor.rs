use renderdocx::shader_processor::{ShaderProcessor, run};

#[test]
#[ignore = "requires RENDERDOCX_TEST_SHADER_PROCESSOR built by the local CMake fixture"]
fn subprocess_supports_stdin_stdout_files_and_failures() {
    let executable = std::env::var_os("RENDERDOCX_TEST_SHADER_PROCESSOR")
        .expect("set RENDERDOCX_TEST_SHADER_PROCESSOR")
        .into();
    let mut tool = ShaderProcessor {
        executable,
        input_encoding: 5,
        output_encoding: 1,
        arguments: vec!["{stdin}".into()],
    };
    let source = "中文 shader\n".as_bytes();
    let result = run(&tool, source, 4, "main", "spirv1.3").unwrap();
    assert_eq!(result.bytes, source);
    assert!(result.diagnostics.contains("fixture compiler diagnostics"));
    tool.arguments = vec!["{input_file}".into(), "{output_file}".into()];
    assert_eq!(run(&tool, source, 4, "main", "").unwrap().bytes, source);
    tool.arguments = vec!["--fail".into()];
    assert!(
        run(&tool, source, 4, "main", "")
            .unwrap_err()
            .contains("fixture compiler failed")
    );
}
