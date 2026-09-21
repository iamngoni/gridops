//! Shared provisioning decisions and lifecycle obligations.
//!
//! The API and reconciler own their I/O adapters, credentials, and persistence
//! details. This module owns the decisions that must not drift between them.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisioningPlan {
    pub provider: String,
    pub platform: &'static str,
    pub mode: String,
    pub ephemeral: bool,
    pub runtime: String,
    pub image: String,
}

impl ProvisioningPlan {
    pub fn for_selected_provider(
        provider: &str,
        bitbucket_connection_selected: bool,
        pool_mode: &str,
        pool_ephemeral: bool,
        macos_runtime: &str,
        docker_image: &str,
        tart_image: &str,
    ) -> Self {
        let platform = if bitbucket_connection_selected {
            "bitbucket"
        } else {
            "github"
        };
        let mode = if bitbucket_connection_selected {
            "persistent".to_owned()
        } else {
            pool_mode.to_owned()
        };
        let ephemeral = platform == "github" && pool_ephemeral;
        let runtime = if provider == "tart" && platform == "github" {
            macos_runtime.to_owned()
        } else {
            "vm".to_owned()
        };
        let image = if provider == "tart" {
            if runtime == "native" {
                String::new()
            } else {
                tart_image.to_owned()
            }
        } else {
            docker_image.to_owned()
        };

        Self {
            provider: provider.to_owned(),
            platform,
            mode,
            ephemeral,
            runtime,
            image,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureCleanup {
    pub release_capacity: bool,
    pub remove_provider_runner: bool,
    pub remove_manager_runner: bool,
}

/// The common failure contract after capacity has been reserved. Adapters can
/// add their own persistence/event handling, but must release the lease and
/// clean up anything already registered outside the database.
pub fn failure_cleanup(
    capacity_reserved: bool,
    provider_runner_registered: bool,
    manager_runner_created: bool,
) -> FailureCleanup {
    FailureCleanup {
        release_capacity: capacity_reserved,
        remove_provider_runner: provider_runner_registered,
        remove_manager_runner: manager_runner_created,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(provider: &str, bitbucket: bool) -> ProvisioningPlan {
        ProvisioningPlan::for_selected_provider(
            provider,
            bitbucket,
            "ephemeral",
            true,
            "native",
            "docker:latest",
            "tart:latest",
        )
    }

    #[test]
    fn api_and_reconciler_inputs_produce_the_same_github_plan() {
        assert_eq!(
            plan("tart", false),
            ProvisioningPlan::for_selected_provider(
                "tart",
                false,
                "ephemeral",
                true,
                "native",
                "docker:latest",
                "tart:latest",
            )
        );
        assert_eq!(plan("tart", false).platform, "github");
        assert_eq!(plan("tart", false).runtime, "native");
        assert!(plan("tart", false).image.is_empty());
    }

    #[test]
    fn bitbucket_is_always_persistent_and_vm_hosted() {
        let selected = plan("tart", true);
        assert_eq!(selected.platform, "bitbucket");
        assert_eq!(selected.mode, "persistent");
        assert!(!selected.ephemeral);
        assert_eq!(selected.runtime, "vm");
        assert_eq!(selected.image, "tart:latest");
    }

    #[test]
    fn failure_contract_only_cleans_up_resources_that_exist() {
        assert_eq!(
            failure_cleanup(true, true, false),
            FailureCleanup {
                release_capacity: true,
                remove_provider_runner: true,
                remove_manager_runner: false,
            }
        );
        assert_eq!(
            failure_cleanup(true, false, true),
            FailureCleanup {
                release_capacity: true,
                remove_provider_runner: false,
                remove_manager_runner: true,
            }
        );
    }
}
