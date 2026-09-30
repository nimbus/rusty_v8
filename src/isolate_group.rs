// Copyright 2018-2019 the Deno authors. All rights reserved. MIT license.
use crate::support::Opaque;
use crate::support::UniqueRef;

unsafe extern "C" {
  fn v8__IsolateGroup__GetDefault() -> *mut IsolateGroup;
  fn v8__IsolateGroup__Create() -> *mut IsolateGroup;
  fn v8__IsolateGroup__CanCreateNewGroups() -> bool;
  fn v8__IsolateGroup__DELETE(this: *mut IsolateGroup);
}

/// A group of isolates sharing a pointer-compression cage and a private
/// read-only heap. With `v8_enable_pointer_compression_shared_cage=false`, V8
/// supports run-time creation of multiple groups; placing each isolate in its
/// own group gives it a private shared-RO-heap, eliminating the cross-thread
/// snapshot-deserialization race on the default group's shared RO heap.
///
/// Reference-counted by V8; a `UniqueRef<IsolateGroup>` owns one ref and drops
/// it on `Drop`. The group's virtual-memory reservation is released only once
/// the last isolate in it is disposed AND no live handle remains.
#[repr(C)]
#[derive(Debug)]
pub struct IsolateGroup(Opaque);

impl IsolateGroup {
  /// The default group. Requires `V8::initialize()` to have run.
  #[inline(always)]
  pub fn get_default() -> UniqueRef<IsolateGroup> {
    crate::V8::assert_initialized();
    unsafe { UniqueRef::from_raw(v8__IsolateGroup__GetDefault()) }
  }

  /// Create a new isolate group. Aborts on a single-cage build -- gate on
  /// `can_create_new_groups()`. Requires `V8::initialize()` to have run.
  #[inline(always)]
  pub fn create() -> UniqueRef<IsolateGroup> {
    crate::V8::assert_initialized();
    unsafe { UniqueRef::from_raw(v8__IsolateGroup__Create()) }
  }

  /// True if new groups can be created at run time (multi-cage build).
  /// `static constexpr` in V8 -- safe to call before initialization.
  #[inline(always)]
  pub fn can_create_new_groups() -> bool {
    unsafe { v8__IsolateGroup__CanCreateNewGroups() }
  }
}

impl Drop for IsolateGroup {
  fn drop(&mut self) {
    unsafe { v8__IsolateGroup__DELETE(self) };
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn can_query_groups() {
    // Safe pre-init (static constexpr). Don't call create() here: it aborts on
    // single-cage builds. Cross-group concurrency is covered by nimbus's
    // pool_reuse acceptance test on the multi-cage build.
    let _ = super::IsolateGroup::can_create_new_groups();
  }
}
