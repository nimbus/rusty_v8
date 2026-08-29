// Copyright 2019-2020 the Deno authors. All rights reserved. MIT license.
// Test that Locker is not Send - it cannot be transferred to another thread.
// A live Locker must stay on the thread that constructed it.

pub fn main() {
  let mut isolate = unsafe { v8::Isolate::new_unentered(mock()) };
  let locker = v8::Locker::new(&mut isolate);

  std::thread::scope(|scope| {
    // Error: Locker is not Send. The scoped thread removes any unrelated
    // `'static` lifetime requirement from this proof.
    scope.spawn(move || {
      drop(locker);
    });
  });
}

fn mock<T>() -> T {
  unimplemented!()
}
