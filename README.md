# Amalthea

## About

Experimental kernel for Jupyter and Myriac, written in Rust. 

During the prototyping phase, this kernel will implement a trivial echo-based language. Later, it will be factored into a [Rust Crate](https://doc.rust-lang.org/book/ch07-01-packages-and-crates.html) library that has no language implementation at all, but provides the shared Jupyter functionality necessary to build Rust-based kernels for other languages. This crate will then be used as the basis for Python and R kernels. 

### Why not Xeus?

The [Xeus](https://github.com/jupyter-xeus/xeus) project supplies all the nuts and bolts of Jupyter kernel communication, with the goal of letting kernel implementors focus only on the actual language bindings. 

Unfortunately this project cuts across architectural boundaries in ways that make it hard to extend with Rust. For example, Xeus depends on an army of C/C++ libraries, some header-only, that provide its JSON and ZeroMQ functionality; consequently extending it with Rust requires either marshaling structured data across the language boundary (difficult and tedious) or using multiple, possibly incompatible, libraries that serve the same purpose in the same binary.

Building in pure Rust dramatically simplifies the development environment and lets us standardize on idiomatic Rust tools like `serde_json`. It also shortens the distance to compiling for WASM, a door we'd like to leave open for investigation into browser-only versions of Myriac (a la vscode.dev).

### Features

[x] Jupyter protocol implementation via ZeroMQ
[x] Type-safe Rust structures/enums for (subset of) Jupyter messages
[x] Heartbeats
[x] Shell socket (only)
[x] Kernel info reply/request

### Up Next

[ ] HMAC signatures on messages
[ ] Simple "execution"

### What's with the name?

This is a Jupyter kernel; Amalthea is [one of Jupiter's moons](https://en.wikipedia.org/wiki/Amalthea_(moon)).

## Related

[Xeus](https://github.com/jupyter-xeus/xeus), a C++ base/reference kernel implementation

[IRKernel](https://github.com/IRkernel/IRkernel), a kernel for R written primarily in R itself

[EvCxR Kernel](https://github.com/google/evcxr/tree/main/evcxr_jupyter), a kernel for Rust written in Rust

[Myriac Console](https://github.com/rstudio/myriac-console), an experimental Jupyter front end


