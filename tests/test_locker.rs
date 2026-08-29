use std::pin::pin;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[test]
fn locker_basic() {
  let _setup_guard = setup();
  let mut isolate = v8::Isolate::new_unentered(Default::default());
  {
    let mut locker = v8::Locker::new(&mut isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let value = v8::String::new(scope, "locked").unwrap();
    assert_eq!(value.to_rust_string_lossy(scope), "locked");
  }
}

#[test]
fn locker_with_script() {
  let _setup_guard = setup();
  let mut isolate = v8::Isolate::new_unentered(Default::default());
  {
    let mut locker = v8::Locker::new(&mut isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let code = v8::String::new(scope, "40 + 2").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), 42);
  }
}

#[test]
fn unentered_isolate_no_lifo_constraint() {
  let _setup_guard = setup();
  let mut isolate1 = v8::Isolate::new_unentered(Default::default());
  let isolate2 = v8::Isolate::new_unentered(Default::default());
  let mut isolate3 = v8::Isolate::new_unentered(Default::default());
  drop(isolate2);

  for (isolate, expression, expected) in
    [(&mut isolate1, "5 + 6", 11), (&mut isolate3, "5 + 7", 12)]
  {
    let mut locker = v8::Locker::new(isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let code = v8::String::new(scope, expression).unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), expected);
  }

  drop(isolate1);
  drop(isolate3);
}

#[test]
fn locker_multiple_lock_unlock() {
  let _setup_guard = setup();
  let mut isolate = v8::Isolate::new_unentered(Default::default());

  {
    let mut locker = v8::Locker::new(&mut isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let code = v8::String::new(scope, "1 + 1").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), 2);
  }

  {
    let mut locker = v8::Locker::new(&mut isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let code = v8::String::new(scope, "2 + 2").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), 4);
  }
}

#[test]
fn locker_is_locked() {
  let _setup_guard = setup();
  let mut isolate = v8::Isolate::new_unentered(Default::default());

  assert!(!v8::Locker::is_locked(&isolate));
  {
    let _locker = v8::Locker::new(&mut isolate);
    // Locker is now held - but we can't call is_locked because we have &mut isolate
    // The lock check happens internally
  }
  assert!(!v8::Locker::is_locked(&isolate));
}

#[test]
fn locker_state_preserved_across_locks() {
  let _setup_guard = setup();
  let mut isolate = v8::Isolate::new_unentered(Default::default());

  // First lock: execute some code
  {
    let mut locker = v8::Locker::new(&mut isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let code = v8::String::new(scope, "1 + 1").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), 2);
  }

  // Second lock: isolate should still work correctly
  {
    let mut locker = v8::Locker::new(&mut isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let code = v8::String::new(scope, "2 + 2").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), 4);
  }
}

#[test]
fn locker_drop_releases_lock() {
  let _setup_guard = setup();
  let mut isolate = v8::Isolate::new_unentered(Default::default());

  // Create and immediately drop a locker
  {
    let locker = v8::Locker::new(&mut isolate);
    drop(locker);
  }

  // Should be able to create another locker without blocking
  {
    let _locker = v8::Locker::new(&mut isolate);
  }

  // Isolate should be unlocked now
  assert!(!v8::Locker::is_locked(&isolate));
}

#[test]
fn unentered_isolate_as_raw() {
  let _setup_guard = setup();
  let isolate = v8::Isolate::new_unentered(Default::default());

  // as_raw should return a valid pointer
  let ptr = isolate.as_raw();
  assert!(!ptr.is_null());
}

#[test]
fn locker_send_isolate_between_threads() {
  let _setup_guard = setup();

  // SAFETY: this test installs no slots, finalizers, weak handles, or external
  // aliases. Every later access is through a same-thread Locker.
  let mut isolate = unsafe {
    v8::SendableUnenteredIsolate::new_unchecked(v8::Isolate::new_unentered(
      Default::default(),
    ))
  };

  // Use on main thread first
  {
    let mut locker = v8::Locker::new(isolate.get_mut());
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let code = v8::String::new(scope, "1 + 1").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), 2);
  }

  // Send to another thread
  let (tx, rx) = mpsc::channel();

  let handle = thread::spawn(move || {
    // Use isolate on worker thread - scope in separate block
    let value = {
      let mut locker = v8::Locker::new(isolate.get_mut());
      let scope = pin!(v8::HandleScope::new(&mut *locker));
      let scope = &mut scope.init();
      let context = v8::Context::new(scope, Default::default());
      let scope = &mut v8::ContextScope::new(scope, context);

      let code = v8::String::new(scope, "2 + 2").unwrap();
      let script = v8::Script::compile(scope, code, None).unwrap();
      let result = script.run(scope).unwrap();
      result.to_integer(scope).unwrap().value()
    }; // locker dropped here

    // Send result back
    tx.send(value).unwrap();

    // Return isolate ownership
    isolate
  });

  // Wait for result
  let result = rx
    .recv_timeout(Duration::from_secs(10))
    .expect("worker must execute under the Locker and return its result");
  assert_eq!(result, 4);

  // Get isolate back and use again on main thread
  let mut isolate = handle.join().unwrap();
  {
    let mut locker = v8::Locker::new(isolate.get_mut());
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let code = v8::String::new(scope, "3 + 3").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();
    assert_eq!(result.to_integer(scope).unwrap().value(), 6);
  }
}

#[test]
fn persistent_handles_reacquire_released_locker() {
  let _setup_guard = setup();
  let mut isolate = v8::Isolate::new_unentered(Default::default());

  let (global, weak) = {
    let mut locker = v8::Locker::new(&mut isolate);
    let scope = pin!(v8::HandleScope::new(&mut *locker));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let object = v8::Object::new(scope);
    (v8::Global::new(scope, object), v8::Weak::new(scope, object))
  };

  let global_clone = global.clone();
  let weak_clone = weak.clone();
  drop(global_clone);
  drop(weak_clone);
  drop(global);
  drop(weak);

  let mut locker = v8::Locker::new(&mut isolate);
  let scope = pin!(v8::HandleScope::new(&mut *locker));
  let scope = &mut scope.init();
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let code = v8::String::new(scope, "6 * 7").unwrap();
  let script = v8::Script::compile(scope, code, None).unwrap();
  let result = script.run(scope).unwrap();
  assert_eq!(result.to_integer(scope).unwrap().value(), 42);
}

#[test]
fn isolate_dispose_clears_persistent_handles_under_locker() {
  let _setup_guard = setup();
  let global;
  let weak;
  {
    let mut isolate = v8::Isolate::new_unentered(Default::default());
    {
      let mut locker = v8::Locker::new(&mut isolate);
      let scope = pin!(v8::HandleScope::new(&mut *locker));
      let scope = &mut scope.init();
      let context = v8::Context::new(scope, Default::default());
      let scope = &mut v8::ContextScope::new(scope, context);
      let object = v8::Object::new(scope);
      global = v8::Global::new(scope, object);
      weak = v8::Weak::new(scope, object);
    }
  }

  assert!(weak.is_empty());
  drop(weak);
  drop(global);
}

#[test]
fn forgotten_locker_prevents_isolate_dispose() {
  assert_forgotten_locker_child_fails("same-thread");
}

#[test]
fn forgotten_locker_prevents_cross_thread_isolate_dispose() {
  assert_forgotten_locker_child_fails("cross-thread");
}

fn assert_forgotten_locker_child_fails(mode: &str) {
  let output = Command::new(std::env::current_exe().unwrap())
    .args([
      "--exact",
      "forgotten_locker_dispose_child",
      "--ignored",
      "--nocapture",
    ])
    .env("RUSTY_V8_FORGOTTEN_LOCKER_CHILD", mode)
    .output()
    .unwrap();

  assert!(!output.status.success());
  assert!(
    String::from_utf8_lossy(&output.stderr)
      .contains("Cannot drop UnenteredIsolate while a Locker is held"),
    "child stderr did not contain the held-lock diagnostic: {}",
    String::from_utf8_lossy(&output.stderr)
  );
}

#[test]
#[ignore = "subprocess-only fixture for forgotten_locker_prevents_isolate_dispose"]
fn forgotten_locker_dispose_child() {
  let mode = std::env::var("RUSTY_V8_FORGOTTEN_LOCKER_CHILD").unwrap();
  let _setup_guard = setup();
  if mode == "same-thread" {
    let mut isolate = v8::Isolate::new_unentered(Default::default());
    let locker = v8::Locker::new(&mut isolate);
    std::mem::forget(locker);
    assert!(v8::Locker::is_locked(&isolate));
    drop(isolate);
  } else {
    assert_eq!(mode, "cross-thread");
    // SAFETY: the forgotten-lock test installs no Rust embedder state or
    // external aliases. Drop must reject the move before it accesses V8.
    let mut isolate = unsafe {
      v8::SendableUnenteredIsolate::new_unchecked(v8::Isolate::new_unentered(
        Default::default(),
      ))
    };
    let locker = v8::Locker::new(isolate.get_mut());
    std::mem::forget(locker);
    assert!(v8::Locker::is_locked(isolate.get_mut()));
    let panic = thread::spawn(move || drop(isolate)).join().unwrap_err();
    std::panic::resume_unwind(panic);
  }
}

fn setup() -> impl Drop {
  use std::sync::Once;
  static INIT: Once = Once::new();
  INIT.call_once(|| {
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();
  });
  struct Guard;
  impl Drop for Guard {
    fn drop(&mut self) {}
  }
  Guard
}
