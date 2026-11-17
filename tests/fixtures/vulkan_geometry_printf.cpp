/******************************************************************************
 * The MIT License (MIT)
 *
 * Copyright (c) 2021-2026 Baldur Karlsson
 * Copyright (c) RenderDocX contributors
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 ******************************************************************************/

// Uses RenderDoc's upstream VulkanGraphicsTest harness and helpers.
#include "vk_test.h"

RD_TEST(VK_RenderDocX_Geometry_Printf, VulkanGraphicsTest)
{
  static constexpr const char *Description =
      "Checks geometry printf locations and geometry shader output navigation.";

  void Prepare(int argc, char **argv)
  {
    features.geometryShader = VK_TRUE;
    features.vertexPipelineStoresAndAtomics = VK_TRUE;
    devExts.push_back(VK_KHR_SHADER_NON_SEMANTIC_INFO_EXTENSION_NAME);
    VulkanGraphicsTest::Prepare(argc, argv);
  }

  int main()
  {
    if(!Init())
      return 3;

    const std::string vertex = R"EOSHADER(
#version 450 core
layout(location = 0) in vec3 Position;
void main() { gl_Position = vec4(Position, 1.0); }
)EOSHADER";
    const std::string geometry = R"EOSHADER(
#version 450 core
#extension GL_EXT_debug_printf : require
layout(triangles) in;
layout(triangle_strip, max_vertices = 6) out;
void main()
{
  debugPrintfEXT("rdx-geometry:%d", gl_PrimitiveIDIn);
  for(int i = 0; i < 3; ++i) { gl_Position = gl_in[i].gl_Position; EmitVertex(); }
  EndPrimitive();
  // Primitive zero emits another triangle, so topology-based row 3 is not
  // primitive one's first output. This exposes the upstream approximation.
  if(gl_PrimitiveIDIn == 0)
  {
    for(int i = 0; i < 3; ++i) { gl_Position = gl_in[i].gl_Position + vec4(0.125, 0, 0, 0); EmitVertex(); }
    EndPrimitive();
  }
}
)EOSHADER";
    const std::string pixel = R"EOSHADER(
#version 450 core
layout(location = 0) out vec4 Color;
void main() { Color = vec4(0.0, 1.0, 0.0, 1.0); }
)EOSHADER";

    VkPipelineLayout layout = createPipelineLayout(vkh::PipelineLayoutCreateInfo({}));
    vkh::GraphicsPipelineCreateInfo pipeInfo;
    pipeInfo.layout = layout;
    pipeInfo.renderPass = mainWindow->rp;
    pipeInfo.vertexInputState.vertexBindingDescriptions = {vkh::vertexBind(0, DefaultA2V)};
    pipeInfo.vertexInputState.vertexAttributeDescriptions = {vkh::vertexAttr(0, 0, DefaultA2V, pos)};
    pipeInfo.stages = {
        CompileShaderModule(vertex, ShaderLang::glsl, ShaderStage::vert, "main"),
        CompileShaderModule(geometry, ShaderLang::glsl, ShaderStage::geom, "main"),
        CompileShaderModule(pixel, ShaderLang::glsl, ShaderStage::frag, "main"),
    };
    VkPipeline pipe = createGraphicsPipeline(pipeInfo);

    const uint16_t indices[] = {0, 1, 2, 1, 2, 0};
    AllocatedBuffer vb(this, vkh::BufferCreateInfo(sizeof(DefaultTri), VK_BUFFER_USAGE_VERTEX_BUFFER_BIT),
                       VmaAllocationCreateInfo({0, VMA_MEMORY_USAGE_CPU_TO_GPU}));
    AllocatedBuffer ib(this, vkh::BufferCreateInfo(sizeof(indices), VK_BUFFER_USAGE_INDEX_BUFFER_BIT),
                       VmaAllocationCreateInfo({0, VMA_MEMORY_USAGE_CPU_TO_GPU}));
    vb.upload(DefaultTri);
    ib.upload(indices);

    while(Running())
    {
      VkCommandBuffer cmd = GetCommandBuffer();
      vkBeginCommandBuffer(cmd, vkh::CommandBufferBeginInfo());
      VkImage swapimg = StartUsingBackbuffer(cmd, VK_ACCESS_TRANSFER_WRITE_BIT, VK_IMAGE_LAYOUT_GENERAL);
      vkCmdClearColorImage(cmd, swapimg, VK_IMAGE_LAYOUT_GENERAL,
                           vkh::ClearColorValue(0.2f, 0.2f, 0.2f, 1.0f), 1,
                           vkh::ImageSubresourceRange());
      vkCmdBeginRenderPass(cmd,
                           vkh::RenderPassBeginInfo(mainWindow->rp, mainWindow->GetFB(), mainWindow->scissor),
                           VK_SUBPASS_CONTENTS_INLINE);
      vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipe);
      vkCmdSetViewport(cmd, 0, 1, &mainWindow->viewport);
      vkCmdSetScissor(cmd, 0, 1, &mainWindow->scissor);
      vkh::cmdBindVertexBuffers(cmd, 0, {vb.buffer}, {0});
      vkCmdBindIndexBuffer(cmd, ib.buffer, 0, VK_INDEX_TYPE_UINT16);
      vkCmdDrawIndexed(cmd, 6, 1, 0, 0, 0);
      vkCmdEndRenderPass(cmd);
      FinishUsingBackbuffer(cmd, VK_ACCESS_TRANSFER_WRITE_BIT, VK_IMAGE_LAYOUT_GENERAL);
      vkEndCommandBuffer(cmd);
      Submit(0, 1, {cmd});
      Present();
    }
    return 0;
  }
};

REGISTER_TEST();
