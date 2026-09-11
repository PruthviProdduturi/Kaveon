ARG BASE_IMAGE
FROM ${BASE_IMAGE}
WORKDIR /qualification
COPY aks_distributed_compare.py /qualification/aks_distributed_compare.py
USER 10001:10001
ENTRYPOINT ["python", "/qualification/aks_distributed_compare.py"]
