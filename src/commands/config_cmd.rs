use crate::context::Ctx;
use anyhow::Result;

/// Read-only in v1: shows which config keys exist, never values.
/// Mutation stays manual per playbook § 3.
pub fn run(env: Option<String>) -> Result<()> {
    let ctx = Ctx::load(env)?;
    let ssh = ctx.ssh()?;
    let app = ctx.app();
    let k = ctx.kubectl();
    // go-template braces are kept out of format! via a plain constant
    let tpl = "go-template='{{range $k, $v := .data}}{{$k}}{{\"\\n\"}}{{end}}'";
    ssh.run(&format!(
        "echo '# configmap {app}-config'; {k} get configmap {app}-config -o {tpl} 2>/dev/null || echo '(not found)'; \
         echo; echo '# secret {app}-secret (keys only)'; {k} get secret {app}-secret -o {tpl} 2>/dev/null || echo '(not found)'"
    ))
}
