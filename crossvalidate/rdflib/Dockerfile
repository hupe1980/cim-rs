# rdflib, containerised, as an independent parser of cim-rs's RDF export.
#
# Pinned for the same reason the PowSyBl image is: a reference implementation that moves on
# its own turns a cross-validation failure into somebody else's changelog.

FROM python:3.12-slim
RUN pip install --no-cache-dir "rdflib==7.1.4"
COPY summarize.py /summarize.py
ENTRYPOINT ["python", "/summarize.py"]
