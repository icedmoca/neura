# Vendored Subtext

Upstream: https://github.com/ninjahawk/Subtext (Apache-2.0)
Vendored at commit: 75925193f4308ed4d7ce0af78cfa83cccc121544

Subtext streams live Jacobian-lens "silent words" (J-space) for a local model
over a websocket. Neura's subtext observer (src/agent/subtext_observer.rs)
auto-discovers this vendored copy at $NEURA_HOME/vendor/subtext (installed by
install.sh) and starts it on demand; when its Python environment or model is
unavailable, Neura degrades to the logit-lens service, then the local OSS
model narrator, then deterministic pipeline-stage narration — the thought
stream never goes silent.

Excluded from vendoring: media/ and docs/ (~10 MB of demo assets not needed
at runtime). First run creates a virtualenv from requirements.txt and
downloads the model/lens weights from Hugging Face.
