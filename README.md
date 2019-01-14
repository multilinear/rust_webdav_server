# rust_webdav_server
An asynchronous WebDAV server written in rust. 

This is not complete yet. I'm only pushing code that compiles, but this is not nearly to the level of a fully usable webdav server. 
https://github.com/tylerwhall/hyperdav-server is a more complete webdav server. Though it's not async, and not terribly compliant.

The goal is to implement a real, produciton level webdav server that:
1) meets https://tools.ietf.org/html/rfc4918
2) actually works with some useful clients
3) has CalDAV and CarDAV support as well
4) is written in rust (obviously)

When reviewing the state of WebDAV servers, I found that almost all are written in dynamic languages like PhP or Python.
As a result their performance, reliability, stability, and security is lackluster. So, the goal is to fix that situation.

Note: This is my first rust project. It's not intended to be a toy, but I may do some stupid things :P.
