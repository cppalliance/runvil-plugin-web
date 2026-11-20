# Web Plugin

This crate is the web Plugin, built on `runvil-plugin-sdk` as a dynamic library for a Host's Plugin directory. It inserts the Pack `runvil/web`, which gives a prompt two tools: a search tool, `web/search`, and a fetch tool, `web/fetch`.

The fetch tool is the security boundary between a model-chosen URL and the network: it checks every URL that a model or a redirect names, it revalidates every resolved address on every hop, and it refuses non-public destinations. The Plugin is handed no configuration, so nothing grants an exception.

The search tool runs no search of its own. It sends each search to the Gateway, at the address a Host puts in the `runvil/web-search` entry of its resources, with the Gateway's key when the entry holds one. So the Plugin holds no key of a search vendor, and it writes the Gateway's key into that request and into nothing else: no output, no error message, and no `Debug` text. Under a Host that offers no entry, the Pack and its fetch tool work all the same, and a search fails when it is called.
