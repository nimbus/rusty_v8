// Copyright 2019-2026 the Deno authors. All rights reserved. MIT license.
// A persistent Weak handle cannot remain on one thread while its isolate is
// transferred to another thread through SendableUnenteredIsolate.

pub fn main() {
  assert_send::<v8::Weak<v8::Value>>();
}

fn assert_send<T: Send>() {}
