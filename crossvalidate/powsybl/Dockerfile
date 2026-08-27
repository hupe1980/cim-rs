# PowSyBl, containerised, as an independent reader of cim-rs's output.
#
# A container rather than a toolchain on the host: cross-validation is only worth anything
# if the reference implementation is pinned, and "install a JDK and Maven, then hope the
# resolved dependency tree matches" is not pinned. It also keeps the check runnable by
# anyone with Docker and no Java at all, which is most of this crate's contributors.

FROM maven:3.9-eclipse-temurin-21 AS build
WORKDIR /src

# Dependencies resolve in their own layer, so editing the Java source does not re-download
# PowSyBl. `go-offline` is best-effort — it fails on some plugin metadata — so the build
# below is what actually guarantees a complete tree.
COPY pom.xml .
RUN mvn -q -B dependency:go-offline || true

COPY src ./src
RUN mvn -q -B package

# A JRE for the run stage: the build image carries Maven and a full JDK, which is about
# 600 MB of nothing this needs.
FROM eclipse-temurin:21-jre
COPY --from=build /src/target/crossvalidate.jar /crossvalidate.jar

# PowSyBl looks for its configuration in a directory it expects to exist and logs a
# warning when it does not; an empty one keeps the output clean enough to parse.
RUN mkdir -p /opt/powsybl/config
ENV POWSYBL_CONFIG_DIRS=/opt/powsybl/config

# Only the summary goes to stdout, so the caller can read it as JSON; SLF4J is told to put
# everything else on stderr.
ENV JAVA_TOOL_OPTIONS="-Dorg.slf4j.simpleLogger.logFile=System.err -Dorg.slf4j.simpleLogger.defaultLogLevel=warn"

ENTRYPOINT ["java", "-jar", "/crossvalidate.jar"]
