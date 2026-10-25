# Objective-C notes example

This example creates a namespace and a `notes` collection, upserts two documents,
then runs hybrid text and vector search with a filter. It targets the upcoming
Loams SDK 0.1.2; installation requires that CocoaPods release to be published.

## Set up

You need macOS 12 or newer, Xcode and CocoaPods. In Xcode, create a macOS
Command Line Tool project named `Notes` with Objective-C as its language and set
the deployment target to macOS 12 or newer. Replace its generated `main.m` with
the [main.m](main.m) in this directory.

In the directory containing your `Notes.xcodeproj`, create a `Podfile`:

```ruby
platform :osx, '12.0'
use_frameworks!

target 'Notes' do
  pod 'Loams', '0.1.2'
end
```

If you chose a different Xcode target name, use that name in the `target` block.
Run `pod install`, then open `Notes.xcworkspace` in Xcode. The pod is named
`Loams`; its Objective-C module is `LoamsObjC`, imported by `@import LoamsObjC;`
in the example. This directory supplies source for your Xcode target, rather
than an Xcode project of its own.

## Run

Start a local engine with `loams dev`. In Xcode's **Product → Scheme → Edit
Scheme → Run → Arguments**, add these environment variables:

| Variable | Value |
| --- | --- |
| `LOAMS_ENDPOINT` | `http://127.0.0.1:8080` |
| `LOAMS_NAMESPACE` | `examples-objc` |

Set `LOAMS_API_KEY` if your engine requires authentication. Build and run the
`Notes` scheme. The example prints matching document keys and titles to the
console and writes its sample data into the `examples-objc` namespace.

You can also run the executable produced by Xcode:

```sh
LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=examples-objc /path/to/Notes
```
