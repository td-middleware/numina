/// AgentExecutor — 将 ActAgent 包装为统一的任务执行接口
///
/// 提供：
/// - execute_task：运行 ReAct 循环，返回最终答案
/// - execute_task_verbose：带详细步骤输出
/// - 内存记录（AgentMemory）

use anyhow::Result;
use super::{Agent, AgentStatus};
use super::memory::AgentMemory;
use super::act::{ActAgent, AgentRunResult};

pub struct AgentExecutor {
    agent: Agent,
    memory: AgentMemory,
}

impl AgentExecutor {
    pub fn new(agent: Agent) -> Self {
        let memory = AgentMemory::new(&agent.id);
        Self { agent, memory }
    }

    /// 执行任务（使用 ActAgent ReAct 循环）
    pub async fn execute_task(&mut self, task: &str) -> Result<String> {
        self.agent.set_status(AgentStatus::Busy);

        // 记录任务到内存
        self.memory.add_entry("task", task).await?;

        // 使用 ActAgent 执行
        let result = self.run_act_agent(task, None, None).await;

        let answer = match result {
            Ok(run_result) => {
                self.memory.add_entry("result", &run_result.final_answer).await?;
                run_result.final_answer
            }
            Err(e) => {
                let err_msg = format!("Agent execution failed: {}", e);
                self.memory.add_entry("error", &err_msg).await?;
                err_msg
            }
        };

        self.agent.set_status(AgentStatus::Idle);
        Ok(answer)
    }

    /// 执行任务，带模型覆盖和工作目录
    pub async fn execute_task_with_opts(
        &mut self,
        task: &str,
        model_override: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<AgentRunResult> {
        self.agent.set_status(AgentStatus::Busy);
        self.memory.add_entry("task", task).await?;

        let result = self.run_act_agent(task, model_override, cwd).await;

        match &result {
            Ok(r) => {
                self.memory.add_entry("result", &r.final_answer).await?;
            }
            Err(e) => {
                self.memory.add_entry("error", &e.to_string()).await?;
            }
        }

        self.agent.set_status(AgentStatus::Idle);
        result
    }

    /// 内部：创建并运行 ActAgent
    async fn run_act_agent(
        &self,
        task: &str,
        model_override: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<AgentRunResult> {
        let act_agent = ActAgent::new()?
            .with_verbose(true)
            .with_max_steps(20);

        act_agent.run(task, model_override, cwd).await
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    pub fn agent_mut(&mut self) -> &mut Agent {
        &mut self.agent
    }
}
