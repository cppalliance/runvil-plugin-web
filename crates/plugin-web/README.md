# Web Plugin

This crate is the web Plugin, built on `runvil-plugin-sdk` as a dynamic library for a Host's Plugin directory. It inserts the Pack `runvil/web`, which gives a prompt a fetch tool, `web/fetch`. The fetch tool is the security boundary between a model-chosen URL and the network: it checks every URL that a model or a redirect names, it revalidates every resolved address on every hop, and it refuses non-public destinations. The Plugin is handed no configuration, so nothing grants an exception.
