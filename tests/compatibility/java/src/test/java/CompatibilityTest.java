import java.net.URI;
import java.util.UUID;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;
import software.amazon.awssdk.auth.credentials.AwsBasicCredentials;
import software.amazon.awssdk.auth.credentials.StaticCredentialsProvider;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.core.checksums.RequestChecksumCalculation;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.*;
import software.amazon.awssdk.services.s3.model.*;

class CompatibilityTest {
    private S3Client client(boolean chunked) {
        return S3Client.builder()
            .endpointOverride(URI.create(System.getenv("RECORD_STORE_COMPAT_ENDPOINT")))
            .region(Region.US_EAST_1)
            .credentialsProvider(StaticCredentialsProvider.create(AwsBasicCredentials.create(
                System.getenv("RECORD_STORE_ROOT_ACCESS_KEY"), System.getenv("RECORD_STORE_ROOT_SECRET_KEY"))))
            .forcePathStyle(true)
            // Explicitly retain the Java default despite the runner's WHEN_REQUIRED environment.
            .requestChecksumCalculation(RequestChecksumCalculation.WHEN_SUPPORTED)
            .serviceConfiguration(S3Configuration.builder().chunkedEncodingEnabled(chunked).build())
            .build();
    }

    @Test
    void javaDefaultsAndUnchunkedUploads() {
        String bucket = "java-" + UUID.randomUUID();
        try (S3Client defaults = client(true); S3Client s3 = client(false)) {
            // No payload-signing override: Java's default unsigned CreateBucket must work.
            defaults.createBucket(b -> b.bucket(bucket));
            try {
                S3Exception error = assertThrows(S3Exception.class, () ->
                    defaults.putObject(b -> b.bucket(bucket).key("unsupported"), RequestBody.fromString("hello world")));
                assertEquals(501, error.statusCode());
                assertEquals("NotImplemented", error.awsErrorDetails().errorCode());
                assertTrue(error.awsErrorDetails().errorMessage().contains("disable chunked encoding"));
                assertFalse(s3.listObjectsV2(b -> b.bucket(bucket)).hasContents());

                s3.putObject(b -> b.bucket(bucket).key("object"), RequestBody.fromString("hello world"));
                assertEquals(11, s3.headObject(b -> b.bucket(bucket).key("object")).contentLength());
                assertEquals("hello world", s3.getObjectAsBytes(b -> b.bucket(bucket).key("object")).asUtf8String());
                assertEquals("world", s3.getObjectAsBytes(b -> b.bucket(bucket).key("object").range("bytes=6-10")).asUtf8String());
                assertEquals("object", s3.listObjectsV2(b -> b.bucket(bucket)).contents().getFirst().key());

                String upload = s3.createMultipartUpload(b -> b.bucket(bucket).key("multipart")).uploadId();
                try {
                    String etag = s3.uploadPart(b -> b.bucket(bucket).key("multipart").uploadId(upload).partNumber(1),
                        RequestBody.fromString("multipart data")).eTag();
                    s3.completeMultipartUpload(b -> b.bucket(bucket).key("multipart").uploadId(upload)
                        .multipartUpload(m -> m.parts(CompletedPart.builder().partNumber(1).eTag(etag).build())));
                    assertEquals("multipart data", s3.getObjectAsBytes(b -> b.bucket(bucket).key("multipart")).asUtf8String());
                } catch (Throwable failure) {
                    s3.abortMultipartUpload(b -> b.bucket(bucket).key("multipart").uploadId(upload));
                    throw failure;
                }
                String aborted = s3.createMultipartUpload(b -> b.bucket(bucket).key("aborted")).uploadId();
                assertTrue(s3.listMultipartUploads(b -> b.bucket(bucket)).uploads().stream()
                    .anyMatch(u -> u.uploadId().equals(aborted)));
                s3.abortMultipartUpload(b -> b.bucket(bucket).key("aborted").uploadId(aborted));
                assertFalse(s3.listMultipartUploads(b -> b.bucket(bucket)).hasUploads());
            } finally {
                for (var object : s3.listObjectsV2(b -> b.bucket(bucket)).contents()) {
                    s3.deleteObject(b -> b.bucket(bucket).key(object.key()));
                }
                s3.deleteBucket(b -> b.bucket(bucket));
            }
        }
    }
}
