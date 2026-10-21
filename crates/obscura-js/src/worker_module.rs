use super::*;
use deno_core::{ModuleLoader, ModuleSpecifier, ModuleLoadReferrer, ModuleLoadOptions, ModuleLoadResponse};
use deno_core::error::ModuleLoaderError;

pub(super) struct WorkerModuleLoader {
    pub inner: crate::module_loader::ObscuraModuleLoader,
    pub owner: SharedState,
    pub entry: Option<Result<crate::local_resource::LocalResource,String>>,
    pub cancellation: std::sync::Arc<WorkerCancellation>,
}
impl ModuleLoader for WorkerModuleLoader {
    fn resolve(&self, specifier: &str, referrer: &str, kind: deno_core::ResolutionKind) -> Result<ModuleSpecifier,ModuleLoaderError> {
        self.inner.resolve(specifier,referrer,kind)
    }
    fn load(&self, url: &ModuleSpecifier, referrer: Option<&ModuleLoadReferrer>, options: ModuleLoadOptions) -> ModuleLoadResponse {
        if self.cancellation.is_cancelled() { return ModuleLoadResponse::Sync(Err(JsErrorBox::type_error("Worker has terminated"))); }
        {
            let mut owner = self.owner.borrow_mut();
            if url.as_str() != owner.url {
                if let Err(error) = policy_check(&owner.content_security_policy,&crate::window::origin(&owner),url,false) { return ModuleLoadResponse::Sync(Err(error)); }
            }
            if url.as_str() == owner.url {
                if let Some(entry) = &self.entry {
                    owner.worker_entry_pending = false;
                    return ModuleLoadResponse::Sync(match entry {
                        Err(error) => Err(JsErrorBox::type_error(error.clone())),
                        Ok(resource) if !crate::local_resource::is_javascript_mime(&resource.mime) => Err(JsErrorBox::type_error("Worker module has an unsupported MIME type")),
                        Ok(resource) => Ok(deno_core::ModuleSource::new(deno_core::ModuleType::JavaScript,
                            deno_core::ModuleSourceCode::String(obscura_net::encoding::decode_non_html(&resource.bytes,Some(&resource.mime)).into()),url,None)),
                    });
                }
            }
        }
        match self.inner.load(url,referrer,options) {
            ModuleLoadResponse::Async(future) => {
                let cancellation = self.cancellation.clone();
                ModuleLoadResponse::Async(Box::pin(async move { tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => Err(JsErrorBox::type_error("Worker has terminated")),
                    result = future => result,
                } }))
            }
            response => response,
        }
    }
}
