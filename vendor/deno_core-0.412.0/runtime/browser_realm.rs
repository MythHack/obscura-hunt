// Copyright 2018-2026 the Deno authors. MIT license.

//! Native module-map ownership for browser contexts in an existing isolate.

use std::cell::Cell;
use std::cell::RefCell;
use std::future::Future;
use std::future::poll_fn;
use std::pin::Pin;
use std::rc::Rc;
use std::rc::Weak;
use std::task::Context;
use std::task::Poll;

use futures::FutureExt;
use futures::StreamExt;
use futures::task::AtomicWaker;

use super::ContextState;
use super::JsRealm;
use super::JsRuntime;
use super::JsRuntimeState;
use super::jsrealm::CONTEXT_STATE_SLOT_INDEX;
use super::jsrealm::MODULE_MAP_SLOT_INDEX;
use super::jsruntime::STATE_DATA_OFFSET;
use crate::ModuleCodeString;
use crate::ModuleId;
use crate::ModuleLoader;
use crate::ModuleSpecifier;
use crate::error::CoreError;
use crate::error::CoreErrorKind;
use crate::error::exception_to_err;
use crate::modules::ModuleMap;
use crate::modules::SideModuleKind;
use crate::modules::recursive_load::RecursiveModuleLoad;

/// A native handle to a real browser context's independent module registry.
///
/// Attachment does not create a context, bootstrap JavaScript, or bind new ops.
/// The supplied context must belong to the current `JsRuntime` isolate, have
/// unused deno_core embedder slots, and use the isolate's default microtask
/// queue. Attach before any author script can call `import()`. Existing async
/// ops and exception reporting share the attaching context's `ContextState`;
/// module compilation, linking, namespaces and imports use the supplied context.
///
/// All scope-taking methods enter the target context themselves. Scopes and
/// handles must belong to the original isolate, which must outlive their use.
/// No method holds an isolate or scope borrow while awaiting loader I/O. The
/// owning runtime polls attached maps in its normal event loop. Embedders must
/// retire a document's handle before replacing or closing that document.
#[derive(Clone)]
pub struct BrowserRealm(pub(crate) Rc<BrowserRealmInner>);

pub(crate) struct BrowserRealmInner {
  context: RefCell<Option<v8::Global<v8::Context>>>,
  module_map: Rc<ModuleMap>,
  runtime: *const JsRuntimeState,
  attached: bool,
  retired: Cell<bool>,
  retirement_waker: AtomicWaker,
  loads: RefCell<Vec<Weak<BrowserLoadState>>>,
}

// Context-owned strong references make the raw embedder pointers safe even
// when a retired document is retained by an old function or global reference.
// This must not hold a Global<Context>: that would prevent context collection.
struct BrowserContextData {
  _context_state: Rc<ContextState>,
  _module_map: Rc<ModuleMap>,
}

type PrepareLoad =
  dyn Future<Output = Result<RecursiveModuleLoad, CoreError>>;

enum LoadPhase {
  Initial {
    specifier: String,
    code: Option<ModuleCodeString>,
  },
  Preparing(Pin<Box<PrepareLoad>>),
  Loading(RecursiveModuleLoad),
}

struct BrowserLoadState {
  phase: RefCell<Option<LoadPhase>>,
  module_map: Rc<ModuleMap>,
  counted: Cell<bool>,
}

impl BrowserLoadState {
  fn release(&self) {
    if self.counted.replace(false) {
      let count = &self.module_map.pending_browser_loads;
      count.set(count.get() - 1);
    }
  }

  fn cancel(&self) {
    self.phase.borrow_mut().take();
    self.release();
  }
}

impl Drop for BrowserLoadState {
  fn drop(&mut self) {
    self.release();
  }
}

/// Owned preparation and graph-loading state, with no borrowed V8 handles.
///
/// Poll with [`BrowserRealm::poll_load`] using a fresh runtime borrow each turn.
/// Dropping a job cancels its loader work; retiring its realm cancels all jobs.
/// The returned `ModuleId` is local to that realm, not to the isolate.
pub struct BrowserModuleLoad {
  realm: BrowserRealm,
  state: Rc<BrowserLoadState>,
}

impl BrowserRealm {
  /// Attach an independent module map to a supplied, unregistered V8 context.
  ///
  /// Panics if deno_core's slots are already occupied. The active scope must
  /// be in an initialized deno_core context in the same isolate, so its shared
  /// async-op state can be captured before entering the supplied context.
  pub fn attach(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
    loader: Rc<dyn ModuleLoader>,
  ) -> Self {
    let runtime = JsRuntime::state_from(scope);
    let context_state = JsRealm::state_from_scope(scope);
    assert!(
      context
        .get_aligned_pointer_from_embedder_data(CONTEXT_STATE_SLOT_INDEX)
        .is_null(),
      "browser context already has deno_core context state"
    );
    assert!(
      context
        .get_aligned_pointer_from_embedder_data(MODULE_MAP_SLOT_INDEX)
        .is_null(),
      "browser context already has a deno_core module map"
    );
    let module_map = Rc::new(ModuleMap::new(
      loader,
      runtime.source_mapper.clone(),
      context_state.exception_state.clone(),
      false,
    ));
    context.set_slot(Rc::new(BrowserContextData {
      _context_state: context_state.clone(),
      _module_map: module_map.clone(),
    }));
    // SAFETY: Context-owned BrowserContextData keeps these Rcs alive until
    // V8 collects the context. Unlike the main realm, these slots do not own
    // raw Rc references and must never be reconstructed with Rc::from_raw.
    unsafe {
      context.set_aligned_pointer_in_embedder_data(
        CONTEXT_STATE_SLOT_INDEX,
        Rc::as_ptr(&context_state) as *mut _,
      );
      context.set_aligned_pointer_in_embedder_data(
        MODULE_MAP_SLOT_INDEX,
        Rc::as_ptr(&module_map) as *mut _,
      );
    }
    let realm = Self::new(
      v8::Global::new(scope, context),
      module_map,
      Rc::as_ptr(&runtime),
      true,
    );
    // Keep a non-owning trace of every attached context. Retired realms leave
    // `browser_realms` while JS may still retain their context (and the slot
    // annex it owns), so isolate cleanup needs its own way to reach them.
    // A plain weak handle does not extend the context's lifetime; collected
    // entries are pruned on the next attach or retire.
    {
      let mut tracked = runtime.attached_contexts.borrow_mut();
      tracked.retain(|tracked| !tracked.is_empty());
      tracked.push(v8::Weak::new(&mut **scope, context));
    }
    runtime.browser_realms.borrow_mut().push(realm.clone());
    runtime.notify_new_dynamic_import();
    realm
  }

  pub(crate) fn new(
    context: v8::Global<v8::Context>,
    module_map: Rc<ModuleMap>,
    runtime: *const JsRuntimeState,
    attached: bool,
  ) -> Self {
    Self(Rc::new(BrowserRealmInner {
      context: RefCell::new(Some(context)),
      module_map,
      runtime,
      attached,
      retired: Cell::new(false),
      retirement_waker: Default::default(),
      loads: Default::default(),
    }))
  }

  fn assert_isolate(&self, scope: &v8::PinScope) {
    assert_eq!(
      scope.get_data(STATE_DATA_OFFSET) as *const JsRuntimeState,
      self.0.runtime,
      "BrowserRealm must be used in its owning JsRuntime isolate"
    );
  }

  fn context<'s>(
    &self,
    scope: &mut v8::PinScope<'s, '_>,
  ) -> Result<v8::Local<'s, v8::Context>, CoreError> {
    self.assert_isolate(scope);
    self.0.context.borrow().as_ref().map_or_else(
      || Err(CoreErrorKind::ExecutionTerminated.into_box()),
      |context| Ok(v8::Local::new(scope, context)),
    )
  }

  /// Begin loading a side-module graph, optionally supplying its root source.
  /// No code is compiled until the first poll with a target-context scope.
  pub fn start_load(
    &self,
    specifier: &ModuleSpecifier,
    code: Option<ModuleCodeString>,
  ) -> BrowserModuleLoad {
    let counted = !self.0.retired.get();
    if counted {
      let count = &self.0.module_map.pending_browser_loads;
      count.set(count.get() + 1);
    }
    let state = Rc::new(BrowserLoadState {
      phase: RefCell::new(counted.then(|| LoadPhase::Initial {
        specifier: specifier.to_string(),
        code,
      })),
      module_map: self.0.module_map.clone(),
      counted: Cell::new(counted),
    });
    let mut loads = self.0.loads.borrow_mut();
    loads.retain(|load| load.upgrade().is_some_and(|s| s.counted.get()));
    loads.push(Rc::downgrade(&state));
    BrowserModuleLoad {
      realm: self.clone(),
      state,
    }
  }

  /// Poll real loader I/O, compile ready sources and instantiate the full graph.
  /// `Pending` releases all scope and isolate borrows. Poll the owning runtime
  /// event loop alongside this job, particularly for loaders using JS hooks.
  pub fn poll_load(
    &self,
    job: &mut BrowserModuleLoad,
    cx: &mut Context,
    scope: &mut v8::PinScope,
  ) -> Poll<Result<ModuleId, CoreError>> {
    assert!(Rc::ptr_eq(&self.0, &job.realm.0), "module job belongs to another realm");
    let context = self.context(scope)?;
    let scope = &mut v8::ContextScope::new(scope, context);
    loop {
      let Some(phase) = job.state.phase.borrow_mut().take() else {
        return Poll::Ready(Err(CoreErrorKind::ExecutionTerminated.into_box()));
      };
      let next = match phase {
        LoadPhase::Initial { specifier, code } => {
          if let Some(code) = code {
            if let Err(error) = self.0.module_map.new_es_module(
              scope, false, specifier.clone().into(), code, false, None,
            ) {
              job.state.release();
              return Poll::Ready(Err(error.into_error(scope, false, false)));
            }
          }
          LoadPhase::Preparing(Box::pin(RecursiveModuleLoad::side(
            specifier, self.0.module_map.clone(), SideModuleKind::Async, None,
          )))
        }
        LoadPhase::Preparing(mut prepare) => match prepare.poll_unpin(cx) {
          Poll::Pending => {
            *job.state.phase.borrow_mut() = Some(LoadPhase::Preparing(prepare));
            return Poll::Pending;
          }
          Poll::Ready(Ok(load)) => LoadPhase::Loading(load),
          Poll::Ready(Err(error)) => {
            job.state.release();
            return Poll::Ready(Err(error));
          }
        },
        LoadPhase::Loading(mut load) => match load.poll_next_unpin(cx) {
          Poll::Pending => {
            *job.state.phase.borrow_mut() = Some(LoadPhase::Loading(load));
            return Poll::Pending;
          }
          Poll::Ready(Some(Ok((request, source)))) => {
            if let Err(error) = load.register_and_recurse(scope, &request, source) {
              job.state.release();
              return Poll::Ready(Err(error.into_error(scope, false, false)));
            }
            LoadPhase::Loading(load)
          }
          Poll::Ready(Some(Err(error))) => {
            job.state.release();
            return Poll::Ready(Err(error));
          }
          Poll::Ready(None) => {
            let id = load.root_module_id().expect("Root module should be loaded");
            let result = self.0.module_map.instantiate_module(scope, id)
              .map(|()| id)
              .map_err(|error| {
                let exception = v8::Local::new(scope, error);
                CoreErrorKind::Js(exception_to_err(scope, exception, false, false))
                  .into_box()
              });
            job.state.release();
            return Poll::Ready(result);
          }
        },
      };
      if self.0.retired.get() {
        job.state.release();
        return Poll::Ready(Err(CoreErrorKind::ExecutionTerminated.into_box()));
      }
      *job.state.phase.borrow_mut() = Some(next);
    }
  }

  /// Begin native module evaluation in this context. The owned completion
  /// future does not borrow the scope; drive the owning runtime event loop.
  /// As with `JsRuntime::mod_evaluate`, evaluate one graph at a time per map.
  pub fn mod_evaluate(
    &self,
    scope: &mut v8::PinScope,
    id: ModuleId,
  ) -> impl Future<Output = Result<(), CoreError>> + use<> {
    let evaluation = self.context(scope).map(|context| {
      let scope = &mut v8::ContextScope::new(scope, context);
      self.0.module_map.mod_evaluate(scope, id)
    });
    let realm = self.clone();
    async move {
      let evaluation = evaluation?;
      let mut evaluation = std::pin::pin!(evaluation);
      poll_fn(|cx| {
        realm.0.retirement_waker.register(cx.waker());
        if realm.0.retired.get() {
          Poll::Ready(Err(CoreErrorKind::ExecutionTerminated.into_box()))
        } else {
          evaluation.as_mut().poll(cx)
        }
      }).await
    }
  }

  /// Return this map's actual namespace after successful evaluation.
  pub fn get_module_namespace(
    &self,
    scope: &mut v8::PinScope,
    id: ModuleId,
  ) -> Result<v8::Global<v8::Object>, CoreError> {
    let context = self.context(scope)?;
    let scope = &mut v8::ContextScope::new(scope, context);
    self.0.module_map.get_module_namespace(scope, id)
  }

  /// Poll imports and module jobs without polling the shared async-op driver.
  /// Normally the owning `JsRuntime` does this automatically. Errors and TLA
  /// deadlock detection remain part of that runtime's normal event loop.
  pub fn poll_modules(
    &self,
    cx: &mut Context,
    scope: &mut v8::PinScope,
  ) -> Poll<Result<(), CoreError>> {
    self.assert_isolate(scope);
    if self.0.retired.get() {
      return Poll::Ready(Ok(()));
    }
    let context = self.context(scope)?;
    let scope = &mut v8::ContextScope::new(scope, context);
    self.0.module_map.poll_progress(cx, scope)?;
    if self.has_pending() { Poll::Pending } else { Poll::Ready(Ok(())) }
  }

  /// Whether this map has an owned graph load or pending module/import jobs.
  pub fn has_pending(&self) -> bool {
    !self.0.retired.get() && self.0.module_map.has_pending_browser_modules()
  }

  /// Cancel this document's module jobs and release its persistent context.
  /// Shared op state, callbacks and sibling contexts are deliberately untouched.
  /// Retained old JS contexts keep a retired, empty map whose imports reject.
  /// The context-owned slot annex is deliberately not cleared here: it owns
  /// the module map that `MODULE_MAP_SLOT_INDEX` borrows, and JS may still
  /// retain and call this context. Cleanup drops every annex while the
  /// isolate handle is still valid instead.
  /// The main-realm handle cannot be retired by the embedder.
  pub fn retire(&self, scope: &mut v8::PinScope) {
    self.assert_isolate(scope);
    assert!(self.0.attached, "the main browser realm is owned by JsRuntime");
    self.retire_inner();
    let state = JsRuntime::state_from(scope);
    state
      .attached_contexts
      .borrow_mut()
      .retain(|tracked| !tracked.is_empty());
    state.notify_new_dynamic_import();
  }

  /// Whether `context` is owned by an attached browser realm.
  ///
  /// An attached context's slot annex owns browser storage (its module map is
  /// borrowed by `MODULE_MAP_SLOT_INDEX`) that must stay valid until V8
  /// collects the context. Embedders must not clear such a context's slots;
  /// contexts that never attached may still be cleared directly.
  pub fn has_attached_storage(context: v8::Local<v8::Context>) -> bool {
    context.get_slot::<BrowserContextData>().is_some()
  }

  /// Drop deno_core's slot annex for a context that is still alive.
  ///
  /// Only call this when the isolate is guaranteed to stay usable for the
  /// rest of the call (runtime cleanup or snapshotting). `Weak::drop` for the
  /// annex then resets its V8 weak handle and removes the guaranteed
  /// finalizer; during isolate disposal neither is possible and V8's teardown
  /// GC would call through freed finalizer storage.
  pub(crate) fn clear_context_slots(context: v8::Local<v8::Context>) {
    // The annex owns the module-map `Rc` these raw pointers borrow. Null them
    // before dropping their owner so nothing can reconstruct an `Rc` from a
    // freed allocation afterwards.
    unsafe {
      context.set_aligned_pointer_in_embedder_data(
        CONTEXT_STATE_SLOT_INDEX,
        std::ptr::null_mut(),
      );
      context.set_aligned_pointer_in_embedder_data(
        MODULE_MAP_SLOT_INDEX,
        std::ptr::null_mut(),
      );
    }
    context.clear_all_slots();
  }

  pub(crate) fn retire_inner(&self) {
    if self.0.retired.replace(true) { return; }
    for load in self.0.loads.borrow_mut().drain(..) {
      if let Some(load) = load.upgrade() { load.cancel(); }
    }
    if self.0.attached { self.0.module_map.destroy(); }
    self.0.context.borrow_mut().take();
    self.0.retirement_waker.wake();
  }

  pub(crate) fn is_retired(&self) -> bool { self.0.retired.get() }

  pub(crate) fn module_map(&self) -> &Rc<ModuleMap> { &self.0.module_map }

  pub(crate) fn find_stalled_top_level_await(
    &self,
    scope: &mut v8::PinScope,
  ) -> Option<Box<crate::error::JsError>> {
    if self.is_retired() { return None; }
    let context = self.context(scope).ok()?;
    let scope = &mut v8::ContextScope::new(scope, context);
    let messages = self.0.module_map.find_stalled_top_level_await(scope);
    let message = v8::Local::new(scope, messages.first()?);
    Some(crate::error::JsError::from_v8_message(scope, message))
  }
}
