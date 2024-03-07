# aws-lc-fips-sys

[![crates.io](https://img.shields.io/crates/v/aws-lc-fips-sys.svg)](https://crates.io/crates/aws-lc-fips-sys)
[![GitHub](https://img.shields.io/badge/GitHub-awslabs%2Faws--lc--rs-blue)](https://github.com/awslabs/aws-lc-rs)

**Autogenerated** low-level AWS-LC FIPS bindings for the Rust programming language. We do not recommend directly relying on these bindings.

[Documentation](https://github.com/aws/aws-lc).

## FIPS

This crate provides bindings to [AWS-LC-FIPS 2.x](https://github.com/aws/aws-lc/tree/fips-2022-11-02), which has completed
FIPS validation testing by an accredited lab and has been submitted to NIST for certification. The static build of AWS-LC-FIPS
is used.

| Supported Targets |
| --- |
| x86_64-unknown-linux-gnu |
| aarch64-unknown-linux-gnu |

Refer to the [NIST Cryptographic Module Validation Program's Modules In Progress List](https://csrc.nist.gov/Projects/cryptographic-module-validation-program/modules-in-process/Modules-In-Process-List)
for the latest status of the static or dynamic AWS-LC Cryptographic Module. A complete list of supported operating environments will be
made available in the vendor security policy once the validation certificate has been issued. We will also update our release notes
and documentation to reflect any changes in FIPS certification status.

## Release Support

This crate pulls in the source code of the latest AWS-LC FIPS branch to build with it. Bindings for platforms we officially support are pre-generated.
The platforms which `aws-lc-fips-sys` builds on is limited to the platforms where the AWS-LC FIPS static build is supported.

### Pregenerated Bindings Availability

CPU|OS
-------------|-------------
x86-64|Linux
arm-64|Linux

### Tested AWS-LC FIPS Build Environments

`aws-lc-fips-sys` currently relies on the AWS-LC FIPS static build, please see our CI documentation at [AWS-LC](https://github.com/aws/aws-lc/tree/main/tests/ci#unit-tests).

## Build Prerequisites

Since this crate builds AWS-LC as a native library, all build tools needed to build AWS-LC are applicable to `aws-lc-fips-sys` as well. This includes Go and Perl, which are hard dependencies for the AWS-LC FIPS build.

[Building AWS-LC](https://github.com/aws/aws-lc/blob/main/BUILDING.md)

If you use a different build combination for FIPS and would like us to support it, please open an issue to us at [AWS-LC](https://github.com/aws/aws-lc/issues/new?assignees=&labels=&template=build-issue.md&title=).

## Security Notification Process

If you discover a potential security issue in *AWS-LC* or *aws-lc-fips-sys*, we ask that you notify AWS
Security via our
[vulnerability reporting page](https://aws.amazon.com/security/vulnerability-reporting/).
Please do **not** create a public GitHub issue.

If you package or distribute *aws-lc-fips-sys*, or use *aws-lc-fips-sys* as part of a large multi-user service,
you may be eligible for pre-notification of future *aws-lc-fips-sys* releases.
Please contact aws-lc-pre-notifications@amazon.com.

## Contribution

See contributing file at [AWS-LC](https://github.com/aws/aws-lc/blob/main/CONTRIBUTING.md)

## Licensing

See license at [AWS-LC](https://github.com/aws/aws-lc/blob/main/LICENSE)