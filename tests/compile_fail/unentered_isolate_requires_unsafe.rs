// Copyright 2019-2026 the Deno authors. All rights reserved. MIT license.
// Global trait access cannot carry a Locker lifetime, so creating an
// unentered isolate requires an explicit unsafe contract.

pub fn main() {
  let _isolate = v8::Isolate::new_unentered(mock());
}

fn mock<T>() -> T {
  unimplemented!()
}
