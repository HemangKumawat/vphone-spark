// vphone-linux placeholder for vphone-cli's VPhonePCCGPURecovery, which restores cloudOS in a
// temporary Virtualization.framework VM to extract its GPU driver. On Linux that restore will run
// in QEMU (vphone-linux's vresearch101 machine); until then, pass --gpu-driver-bundle.
import Foundation

enum VPhonePCCGPURecovery {
    struct Unsupported: Swift.Error, LocalizedError {
        var errorDescription: String? {
            "On Linux, the cloudOS GPU driver cannot be extracted with a temporary VM yet. "
                + "Pass --gpu-driver-bundle /path/to/AppleParavirtGPUMetalIOGPUFamily.bundle."
        }
    }

    static func stage(cloudOSDirectory: URL, into restoreDirectory: URL, expectedPlatformVersion: String) throws {
        throw Unsupported()
    }
}
