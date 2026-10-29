# The `Loams` pod: the Objective-C SDK (sdks/objc), a preview.
#
# It lives at the repository root, like the root Package.swift, because CocoaPods
# resolves source_files against the podspec's directory when linting and against
# the repository root when it fetches the tag, and both must be the same files.
# The sources are the @objcMembers Swift classes SwiftPM builds and tests in
# sdks/objc/Package.swift; Objective-C callers `@import LoamsObjC;`.
# scripts/sdk/sdk-version.py keeps s.version at the release tag;
# sdk-release.yml lints and pushes this file.
Pod::Spec.new do |s|
  s.name         = 'Loams'
  s.version      = '0.1.3'
  s.summary      = 'Official Objective-C client for Loams (preview).'
  s.description  = 'The Loams Objective-C SDK. Pre-release: the API may change between previews; pin the exact version.'
  s.homepage     = 'https://loams.dev'
  s.license      = { :type => 'Apache-2.0', :file => 'sdks/objc/LICENSE' }
  s.author       = { 'Ostrium Labs' => 'security@ostriumlabs.org' }
  s.source       = { :git => 'https://github.com/ostrium-labs/loams.git', :tag => "v#{s.version}" }

  s.ios.deployment_target = '15.0'
  s.osx.deployment_target = '12.0'
  s.swift_versions = ['6.0']
  s.module_name  = 'LoamsObjC'
  s.source_files = 'sdks/objc/Sources/LoamsObjC/**/*.swift'
  s.frameworks   = 'Foundation'
end
