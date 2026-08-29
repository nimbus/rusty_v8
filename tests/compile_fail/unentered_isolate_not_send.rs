// Copyright 2019-2020 the Deno authors. All rights reserved. MIT license.
// An unentered isolate can own non-Send Rust embedder state, so cross-thread
// transfer requires SendableUnenteredIsolate's explicit unsafe contract.

pub fn main() {
  let isolate = unsafe { v8::Isolate::new_unentered(mock()) };
  std::thread::scope(|scope| {
    scope.spawn(move || {
      drop(isolate);
    });
  });
}

fn mock<T>() -> T {
  unimplemented!()
}
