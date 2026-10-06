// Feature detection and version reporting, as `system.hpp` documents.

#include "loams/system.hpp"

#include <algorithm>
#include <condition_variable>
#include <map>
#include <mutex>

#include "loams/client.hpp"
#include "loams/errors/v1/errors.pb.h"
#include "loams/error.hpp"
#include "loams/facade.hpp"
#include "loams/instance/v1/instance.pb.h"
#include "loams/version.hpp"

namespace loams {
namespace {

/// The RPC `Catalogue` costs. One call, no auth, cheap (R5).
constexpr char kGetInstanceRpc[] = "loams.instance.v1.InstanceService/GetInstance";

/// The module names of design §44 §7.2 and the proto package behind each. The
/// map is deliberately small: only the modules a guard is asked about, because a
/// guard on an unknown name is a caller bug and throwing is the useful answer.
const std::map<std::string, std::string>& ModulePackages() {
  static const std::map<std::string, std::string>* const packages =
      new std::map<std::string, std::string>{
          {"instance", "loams.instance.v1"},
          {"live", "loams.live.v1"},
          // Two facade names for one package (design §44 §7.2): `tables` wraps
          // `Query`, `Mutate` and `Deploy`, which are `loams.live.v1`'s.
          {"tables", "loams.live.v1"},
          {"approvals", "loams.approvals.v1"},
          {"devices", "loams.devices.v1"},
          {"operations", "loams.operations.v1"},
          {"notifications", "loams.notifications.v1"},
      };
  return *packages;
}

}  // namespace

std::string PackageForModule(const std::string& module) {
  const auto& packages = ModulePackages();
  const auto found = packages.find(module);
  if (found != packages.end()) {
    return found->second;
  }
  // A full package name is accepted as-is, so a caller who knows the proto
  // package does not have to map it back to a module name first.
  if (module.rfind("loams.", 0) == 0) {
    return module;
  }
  ThrowInternal("", "loams: \"" + module +
                       "\" is not a module this SDK serves; the modules are " +
                       std::string("instance, live, tables, approvals, devices, operations, notifications"));
}

struct SystemModule::Impl {
  Client* client;
  std::mutex mutex;
  std::condition_variable done;
  Catalogue catalogue;
  bool have_catalogue = false;
  bool in_flight = false;
  std::exception_ptr failure;
};

SystemModule::SystemModule(Client* client) : impl_(std::make_unique<Impl>()) { impl_->client = client; }

SystemModule::~SystemModule() = default;
SystemModule::SystemModule(SystemModule&&) noexcept = default;
SystemModule& SystemModule::operator=(SystemModule&&) noexcept = default;

void SystemModule::Invalidate() {
  std::lock_guard<std::mutex> const guard(impl_->mutex);
  impl_->have_catalogue = false;
}

const Catalogue& SystemModule::GetCatalogue() {
  std::unique_lock<std::mutex> lock(impl_->mutex);
  if (impl_->have_catalogue) {
    return impl_->catalogue;
  }
  if (impl_->in_flight) {
    // Another thread is already fetching. Waiting on **its** result is what makes
    // R5's "concurrent readers share one in-flight fetch" true: a hundred threads
    // asking at once cost one `GetInstance`, not a hundred.
    impl_->done.wait(lock, [this] { return !impl_->in_flight; });
    if (impl_->failure) {
      std::exception_ptr failure = impl_->failure;
      impl_->failure = nullptr;
      std::rethrow_exception(failure);
    }
    if (impl_->have_catalogue) {
      return impl_->catalogue;
    }
    ThrowInternal(kGetInstanceRpc, "loams: the catalogue fetch finished without a result");
  }
  impl_->in_flight = true;
  lock.unlock();

  Catalogue fetched;
  std::exception_ptr failure;
  try {
    loams::instance::v1::GetInstanceResponse info;
    impl_->client->Unary(BindingFor(kGetInstanceRpc), loams::instance::v1::GetInstanceRequest(), &info,
                         impl_->client->Encoding());
    for (const auto& service : info.services()) {
      ServiceStatus status;
      status.package = service.package();
      status.version = service.version();
      status.available = service.available();
      status.unstable = service.unstable();
      for (const auto& name : service.services()) {
        status.services.push_back(name);
      }
      if (status.available) {
        fetched.served.push_back(status.package);
      } else {
        fetched.unavailable.push_back(status.package);
      }
      fetched.services.push_back(std::move(status));
    }
    // R9's `missing`: a package this SDK speaks that the instance does not list
    // at all. Different from `unavailable`, which is a package that exists and is
    // switched off.
    for (const std::string& package : ProtoPackages()) {
      const bool listed =
          std::find(fetched.served.begin(), fetched.served.end(), package) != fetched.served.end() ||
          std::find(fetched.unavailable.begin(), fetched.unavailable.end(), package) !=
              fetched.unavailable.end();
      if (!listed) {
        fetched.missing.push_back(package);
      }
    }
  } catch (...) {
    failure = std::current_exception();
  }

  lock.lock();
  if (failure) {
    impl_->failure = failure;
  } else {
    impl_->catalogue = std::move(fetched);
    impl_->have_catalogue = true;
  }
  impl_->in_flight = false;
  lock.unlock();
  impl_->done.notify_all();

  if (failure) {
    std::rethrow_exception(failure);
  }
  return impl_->catalogue;
}

bool SystemModule::Available(const std::string& package_or_module) {
  const std::string package = PackageForModule(package_or_module);
  const Catalogue& catalogue = GetCatalogue();
  return std::find(catalogue.served.begin(), catalogue.served.end(), package) != catalogue.served.end();
}

std::vector<std::string> SystemModule::Served() { return GetCatalogue().served; }

std::vector<std::string> SystemModule::Unavailable() { return GetCatalogue().unavailable; }

void SystemModule::Guard(const std::string& module) {
  if (Available(module)) {
    return;
  }
  // The **same type** a call gets when the caller skips the guard, which is what
  // makes one `catch (const FeatureNotInVariantError&)` cover both (R5). The
  // variant is read out of the catalogue when the server said which one, and left
  // empty when it did not: guessing would be reporting something the server never
  // said.
  const Catalogue& catalogue = GetCatalogue();
  const std::string package = PackageForModule(module);
  std::map<std::string, std::string> metadata;
  for (const ServiceStatus& service : catalogue.services) {
    if (service.package == package && !service.available) {
      // The variant is not in the catalogue, only in the refusal's metadata, so
      // the guard's error carries the package and no variant rather than a
      // fabricated one. A caller that needs the variant gets it by calling.
      break;
    }
  }
  throw FeatureNotInVariantError(
      "", metadata, /*hint=*/std::string(),
      package + " is not served by this instance, so a call to it would answer unimplemented with reason "
                "feature_not_in_variant",
      /*request_id=*/std::string());
}

VersionReport SystemModule::Version() {
  loams::instance::v1::GetInstanceResponse info;
  impl_->client->Unary(BindingFor(kGetInstanceRpc), loams::instance::v1::GetInstanceRequest(), &info,
                       impl_->client->Encoding());

  VersionReport report;
  report.proto_rev = ProtoRev();
  report.server_version = info.server_version();
  for (const std::string& package : info.api_versions()) {
    report.api_versions.push_back(package);
  }
  for (const std::string& package : ProtoPackages()) {
    if (std::find(report.api_versions.begin(), report.api_versions.end(), package) == report.api_versions.end()) {
      report.missing.push_back(package);
    }
  }
  report.compatible = report.missing.empty();
  // R9: a missing package is a **warning, not an exception**. The SDK still works
  // for the modules that are there, and the caller decides what a missing one
  // means — so the warnings are returned rather than thrown.
  for (const std::string& package : report.missing) {
    report.warnings.push_back("this SDK speaks " + package + " and the instance does not serve it; the SDK still "
                             "works for the modules that are there");
  }
  return report;
}

}  // namespace loams